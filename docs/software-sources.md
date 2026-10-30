# Installed software sources

The Programs tab and store-app tab are mutually exclusive. Search, counts, selection,
and export are scoped to the active tab. Linux software centers use their underlying
package databases; installing through a graphical frontend does not create a new source.

| Platform/source | Inventory and removal implementation | Verification in this workspace |
| --- | --- | --- |
| Windows Win32/MSI | Registered desktop installers, including registered WinGet/Chocolatey installs | Existing Windows backend; native Windows runtime not tested here |
| Windows Store / MSIX / AppX | Installed AppX packages, separate Windows Apps tab | Existing backend; native Windows runtime not tested here |
| Windows Scoop | User/global portable manifests, both manifest filenames, custom installation roots | Portable filesystem fixture tests; native Windows runtime not tested here |
| Debian / Ubuntu / Mint | dpkg database, APT removal and suggested autoremove | Linux unit and GUI checks |
| Fedora / RHEL / openSUSE | RPM database; DNF, DNF5, YUM, Zypper or RPM removal | Existing backend; native distro runtime not tested here |
| Arch / Manjaro / EndeavourOS | Pacman database and removal | Existing backend; native distro runtime not tested here |
| Alpine | APK installed database, metadata/file ownership, `apk del` | Parser and administrator-command validation tests |
| Void | XBPS installed packages/file ownership, `xbps-remove` | Parser and administrator-command validation tests |
| Gentoo and Portage derivatives | VDB metadata/file ownership, exact-version unmerge | Parser and administrator-command validation tests |
| Flatpak / Snap | Independent queries, separate store tab | GUI fixtures and read-only host inventory timing |
| Nix | Active entries in the current user's modern profile; exact store-path removal selector | JSON fixture tests, including older array-shaped profile manifests |
| Homebrew on Linux | Installed formulae; dependency filtering; `brew uninstall --formula` | JSON fixture tests |
| AppImage / manual Linux apps | AppImage files and user launchers | Existing backend and GUI fixtures |
| macOS | No native backend yet | Not supported by the current application |

These implementations are not a claim of universal store coverage. Legacy `nix-env`
profiles, declarative NixOS/system packages, rpm-ostree layering, pkgsrc, and standalone
portable packages from other Windows managers need additional adapters. macOS needs a
native platform port before Mac App Store, `.app` bundles, Homebrew casks and MacPorts
can be advertised as supported. Linux code is not a macOS backend.

Interactive optional-source queries run concurrently and publish results as they finish.
Each has a ten-second limit and reports errors persistently. Incomplete scans are not
cached as complete. Successful inventories are retained in memory for 30 seconds on
navigation. Refresh, software changes and system-component setting changes invalidate
that cache. Destructive leftover protection uses a separate fresh platform inventory.

## Upstream format/command references

- [APK installed database](https://wiki.alpinelinux.org/wiki/Apk_spec)
- [XBPS query](https://man.voidlinux.org/xbps-query.1)
- [Portage VDB](https://dev.gentoo.org/~zmedico/portage/doc/man/portage.5.html)
- [Nix profile listing](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-profile-list)
- [Nix profile removal](https://nix.dev/manual/nix/2.34/command-ref/new-cli/nix3-profile-remove)
- [Homebrew inventory JSON](https://docs.brew.sh/Querying-Brew)
- [Scoop directory layout](https://github.com/ScoopInstaller/Scoop/wiki/Scoop-Folder-Layout)
- [APT autoremove](https://manpages.debian.org/bookworm/apt/apt-get.8.en.html)
