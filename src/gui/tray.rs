//! Hunter Mode tray icon (Revo: Hunter Mode's notification-area icon): show the floating
//! target, open CleanCore, or leave Hunter Mode. Linux: a StatusNotifierItem over D-Bus (KDE,
//! GNOME with the AppIndicator extension, most other desktops); Windows: a notification-area
//! icon.

use eframe::egui;
use std::sync::mpsc::{channel, Receiver, Sender};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowTarget,
    OpenMain,
    Exit,
}

/// A 32×32 crosshair, RGBA.
fn crosshair_rgba(size: u32) -> Vec<u8> {
    let c = (size as f32 - 1.0) / 2.0;
    let mut v = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            let (dx, dy) = (x as f32 - c, y as f32 - c);
            let r = (dx * dx + dy * dy).sqrt();
            let ring = (r - size as f32 * 0.34).abs() < 1.6;
            let dot = r < 2.2;
            let tick = (dx.abs() < 1.1 && (dy.abs() > size as f32 * 0.22) && dy.abs() < size as f32 * 0.48)
                || (dy.abs() < 1.1 && (dx.abs() > size as f32 * 0.22) && dx.abs() < size as f32 * 0.48);
            if ring || dot || tick {
                v.extend_from_slice(&[37, 99, 235, 255]);
            } else {
                v.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    v
}

pub struct Tray {
    rx: Receiver<TrayAction>,
    #[cfg(target_os = "linux")]
    handle: ksni::blocking::Handle<LinuxTray>,
    #[cfg(windows)]
    _icon: tray_icon::TrayIcon,
}

impl Tray {
    /// Show the icon. `None` when the desktop has no tray to show it in.
    pub fn start(ctx: &egui::Context) -> Option<Self> {
        let (tx, rx) = channel();
        Self::start_platform(ctx.clone(), tx).map(|p| p(rx))
    }

    pub fn poll(&self) -> Option<TrayAction> {
        self.rx.try_recv().ok()
    }
}

// ---------------------------------------------------------------- Linux

#[cfg(target_os = "linux")]
pub struct LinuxTray {
    tx: Sender<TrayAction>,
    ctx: egui::Context,
}

#[cfg(target_os = "linux")]
impl LinuxTray {
    fn send(&self, a: TrayAction) {
        let _ = self.tx.send(a);
        self.ctx.request_repaint();
    }
}

#[cfg(target_os = "linux")]
impl ksni::Tray for LinuxTray {
    fn id(&self) -> String {
        "cleancore-hunter".into()
    }
    fn title(&self) -> String {
        "CleanCore Hunter Mode".into()
    }
    fn icon_pixmap(&self) -> Vec<ksni::Icon> {
        // ARGB32, network byte order.
        let rgba = crosshair_rgba(32);
        let data = rgba.chunks(4).flat_map(|p| [p[3], p[0], p[1], p[2]]).collect();
        vec![ksni::Icon { width: 32, height: 32, data }]
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: "CleanCore Hunter Mode".into(), description: "Drag the target onto a program's window".into(), ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        self.send(TrayAction::ShowTarget);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::StandardItem;
        let item = |label: &str, a: TrayAction| -> ksni::MenuItem<Self> {
            StandardItem { label: label.into(), activate: Box::new(move |t: &mut Self| t.send(a)), ..Default::default() }.into()
        };
        vec![item("Show the target", TrayAction::ShowTarget), item("Open CleanCore", TrayAction::OpenMain), ksni::MenuItem::Separator, item("Exit Hunter Mode", TrayAction::Exit)]
    }
}

#[cfg(target_os = "linux")]
impl Tray {
    fn start_platform(ctx: egui::Context, tx: Sender<TrayAction>) -> Option<impl FnOnce(Receiver<TrayAction>) -> Self> {
        use ksni::blocking::TrayMethods;
        let handle = LinuxTray { tx, ctx }.spawn().ok()?;
        Some(move |rx| Self { rx, handle })
    }
}

#[cfg(target_os = "linux")]
impl Drop for Tray {
    fn drop(&mut self) {
        let _ = self.handle.shutdown();
    }
}

// ---------------------------------------------------------------- Windows

/// tray-icon / muda keep their event handlers in set-once cells, so they are installed once
/// and forward to whichever Hunter session is current.
#[cfg(windows)]
static CURRENT: std::sync::Mutex<Option<(Sender<TrayAction>, egui::Context)>> = std::sync::Mutex::new(None);

#[cfg(windows)]
fn forward(a: TrayAction) {
    if let Some((tx, ctx)) = CURRENT.lock().unwrap().as_ref() {
        let _ = tx.send(a);
        ctx.request_repaint();
    }
}

#[cfg(windows)]
impl Tray {
    fn start_platform(ctx: egui::Context, tx: Sender<TrayAction>) -> Option<impl FnOnce(Receiver<TrayAction>) -> Self> {
        use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
        use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
        static HANDLERS: std::sync::Once = std::sync::Once::new();
        HANDLERS.call_once(|| {
            MenuEvent::set_event_handler(Some(|e: MenuEvent| {
                forward(if e.id == "show" {
                    TrayAction::ShowTarget
                } else if e.id == "open" {
                    TrayAction::OpenMain
                } else {
                    TrayAction::Exit
                })
            }));
            TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                    forward(TrayAction::ShowTarget);
                }
            }));
        });
        let menu = Menu::new();
        let _ = menu.append(&MenuItem::with_id("show", "Show the target", true, None));
        let _ = menu.append(&MenuItem::with_id("open", "Open CleanCore", true, None));
        let _ = menu.append(&PredefinedMenuItem::separator());
        let _ = menu.append(&MenuItem::with_id("exit", "Exit Hunter Mode", true, None));
        let icon = Icon::from_rgba(crosshair_rgba(32), 32, 32).ok()?;
        let tray = TrayIconBuilder::new().with_tooltip("CleanCore Hunter Mode").with_icon(icon).with_menu(Box::new(menu)).with_menu_on_left_click(false).build().ok()?;
        *CURRENT.lock().unwrap() = Some((tx, ctx));
        Some(move |rx| Self { rx, _icon: tray })
    }
}

#[cfg(windows)]
impl Drop for Tray {
    fn drop(&mut self) {
        *CURRENT.lock().unwrap() = None;
    }
}
