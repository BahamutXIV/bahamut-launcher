# Getting started

[Back to the documentation index](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/README.md)

Download the launcher for your system, then use it to install the game or
select an existing, fully updated 1.23b client. The launcher download does not
include game files; a fresh game installation needs an Internet connection.

## First launch

For SteamOS or Steam Deck, follow the [Flatpak guide](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/flatpak.md) to install
the launcher bundle and choose a game directory. The steps below cover the
release archives.

1. Get the latest download for your system from the
   [releases page](https://github.com/BahamutXIV/bahamut-launcher/releases).
   - **Windows:** extract the whole download into a writable folder, then
     start `bahamut-launcher.exe`.
   - **Linux:** follow [Linux first launch](#linux-first-launch).
   - **macOS:** unzip the download and open `Bahamut Launcher.app`. Safari may
     unzip it automatically. You can move the app to Applications.
2. On Home, choose **Install** to download the game into a new or empty folder.
   The default is `C:\Games\FINAL FANTASY XIV` on Windows or
   `~/Games/FINAL FANTASY XIV` on Linux and macOS. Use **PATH** to change it
   before installing. Keep the game in its own directory, separate from the launcher.
3. If you already have a fully updated 1.23b client, select its folder in
   **Settings > Misc > Install Location** instead. For an older client, use
   **Install** on Home with a different, empty folder.
4. Follow the installation progress on Home. Once it is ready, choose a
   server profile, sign in, and start the game.

The launcher checks disk space before installing. macOS purgeable storage
counts as unavailable space. On macOS, the first launch may request Documents
access for the retail configuration and download cache. A profile that points
to a server on your local network may also prompt for local network access.

If installation fails, see [Install failures](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md#install-failures).

## Platform requirements

| System | Requirements |
|---|---|
| Windows x86_64 | WebView2 Runtime and x86 Microsoft Visual C++ Runtime. The launcher installs missing runtimes when needed. |
| Linux x86_64 | WebKitGTK 4.1, GTK 3, and glibc 2.35 or newer. The tar.gz does not bundle system libraries. The first game launch downloads Wine; see [Linux Wine engine](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/configuration.md#linux-wine-engine). |
| macOS, Apple Silicon or Intel | Universal `Bahamut Launcher.app`, supplied as a ZIP. Sikarugir Wine downloads on first game launch. Apple Silicon needs Rosetta 2 for Wine. |

SteamOS and Steam Deck use the [Flatpak package](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/flatpak.md).
macOS game launch and client extensions remain unverified against a live client.
Check [Platform support](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/extensions.md#platform-support) for
the current limitations. Linux releases require a glibc-based distribution;
musl distributions are unsupported.

On Windows, installing missing prerequisites needs an Internet connection and
may request elevation. The WebView2 bootstrapper downloads its runtime from
Microsoft. Restart Windows yourself if an installer asks you to.

Keep all launcher files together, including `bahamut-loader.exe`,
`bahamut.dll`, and `plugins/`. On Windows, keep them in the extracted folder;
on Linux, keep them in the extracted or installed folder. The macOS app keeps
its files together automatically.

For a macOS keyboard without Print Screen, set Screenshot's `hotkey` to a
value from `f1` through `f9` in
`~/.bahamut-launcher/config/plugins/screenshot/settings.ini`. Screenshot
capture on macOS under Wine is unverified.

## Linux first launch

Extract the tar.gz into a new or empty directory. Do not extract it over an
older launcher. It contains a single `bahamut-launcher/` folder:

```bash
tar -xzf bahamut-launcher-vX.Y.Z-linux-x86_64.tar.gz
cd bahamut-launcher
./install-dependencies.sh --check
```

The optional dependency check reports missing desktop libraries, an outdated
glibc, the selected Wine, Wine's display/font/audio libraries, and Vulkan
loader and driver availability. To install missing packages, run
`./install-dependencies.sh --install`. It shows the distribution's package
command, asks for confirmation, then runs it with `sudo` or `doas`.

Choose how to run the launcher:

- **Run in place:** `./bahamut-launcher`
- **Install for yourself:** `./install.sh`. This places the files in
  `~/.local/lib/bahamut-launcher`, the command in `~/.local/bin`, and an entry
  with icons in the application menu. The menu entry works even if
  `~/.local/bin` is not on `PATH`.
- **Install for everyone:** `sudo ./install.sh`. This uses `/usr/local`.
  `sudo make install` from the extracted folder, or
  `sudo make -C <extracted folder> install` from elsewhere, does the same.

`./install.sh --help` lists prefix and staging options. The script also reads
`PREFIX` and `DESTDIR` from the environment; set the payload directory with
`--pkgdir` only.

### Update or uninstall on Linux

For a copy that runs in place, delete or rename the old `bahamut-launcher/`
folder before extracting the new tar.gz. This removes files no longer shipped
in the release. Your state in `~/.bahamut-launcher` stays in place.

For an installed copy, run `./install.sh` from the new release's folder.
To uninstall, run the installed script with `--uninstall` as the same user,
for example `~/.local/lib/bahamut-launcher/install.sh --uninstall`.
Replacing, reinstalling, or uninstalling the packaged launcher leaves
`~/.bahamut-launcher` intact.

If an older copy keeps settings beside the executable, follow
[Moving a beside-the-launcher tree](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md#moving-a-beside-the-launcher-tree)
before updating or removing it.

### Gentoo and Hyprland

On Gentoo, install the desktop libraries:

```bash
sudo emerge --ask --noreplace net-libs/webkit-gtk:4.1 x11-libs/gtk+:3
```

To use your own Wine, set `BAHAMUT_WINE` to a Wine 7 or newer build with
32-bit support. Run `./install.sh` from the extracted folder, or use the
[Gentoo package template](https://github.com/BahamutXIV/bahamut-launcher/blob/main/packaging/gentoo/README.md) to install the same
release through a local overlay.

The installed launcher appears in menus that read desktop entries, including
fuzzel, wofi, and rofi in `drun` mode. To float its window in Hyprland 0.53 or
newer, add this to `hyprland.conf`:

```ini
windowrule = match:class ^([Bb]ahamut-launcher)$, float on
```

For Hyprland 0.52 or older, use:

```ini
windowrulev2 = float, class:^([Bb]ahamut-launcher)$
```

The pattern matches both `bahamut-launcher` on native Wayland and
`Bahamut-launcher` under XWayland. GTK uses XWayland when built without Wayland
support or when `GDK_BACKEND=x11` is set.

<a id="portable-archives"></a>

## Where your files are stored

The Windows download is portable: settings, logs, and other writable files
stay beside the executable. Move the whole folder together.

The Linux package and macOS app keep writable files in `~/.bahamut-launcher`:
configuration, logs, backups, screenshots, scripts, custom addons, custom DAT
packages, and the managed Wine prefix. macOS also stores its Wine engine there;
WebKit stores WebView data under `~/Library/WebKit`. Linux stores Wine and its
DXVK cache in `runtime/` under the same state directory.

On Linux and macOS, set `BAHAMUT_LAUNCHER_HOME` to an absolute path to use a
different state directory. See [Configuration](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/configuration.md) for exact
paths, settings, extensions, and backups.

The `.bahamut-launcher-package` marker makes a Linux package use the separate
state directory. You may remove it from a folder you extracted yourself to
keep state beside the executable. **Keep the marker in an installed copy:**
`install.sh` replaces or deletes that whole directory. Read
[Linux package layout](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/configuration.md#linux-package-layout) before changing it.

Releases include `LICENSE.md` and library and font notices under `licenses/`.
Keep those notices with the launcher. SHA-256 checksum files are supplied
beside the downloads on the releases page.

## Support

Check [Troubleshooting](https://github.com/BahamutXIV/bahamut-launcher/blob/main/docs/troubleshooting.md), ask in the
[Bahamut Discord](https://discord.gg/PxK5RJYQjm), or report a reproducible
launcher bug in the [issue tracker](https://github.com/BahamutXIV/bahamut-launcher/issues).

Include the launcher version, operating system, steps to reproduce the
problem, and a reviewed, redacted support log. Do not post passwords, session
tokens, or private packet captures.
