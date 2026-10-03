# Creating a DAT overlay

Dats-Overlay loads replacement DAT files from packages without changing the
installed game files. Each replacement uses the same path relative to the
game as the original resource.

This guide covers package creation and testing. See the
[package configuration rules](configuration.md#datsini) for manifest fields,
selection, and priority, or return to the [documentation index](README.md).

## Official overlay package

Every release includes `bahamut-dats-overlay`. This official package is always
enabled and takes priority over custom packages for any paths it contains.
Its current manifest contains no retail DAT changes. See
[Platform support](extensions.md#platform-support) for which launches apply
packages.

Official files are updated with the launcher:

- On Windows, they belong to the signed release inventory. Launcher updates
  replace listed official files while preserving custom packages and unrelated
  player files.
- In the Linux package and portable Linux or macOS trees, they change when a
  newer release is extracted or installed.
- The macOS app reads them from `Contents/Resources/plugins/dats/`; they change
  with a newer app.

In the macOS app and Linux package, custom packages live in the state root's
`plugins/dats/`. A custom package cannot replace the official one.

A missing official package or malformed official manifest is ignored. Play
can still start and load valid custom packages. Official files use launcher
release verification; there is no separate overlay update, verification, or
repair operation.

## Example: package a DAT replacement

Start with a compatible DAT payload and its exact path relative to the client.
The example path `data/1C/59/00/CB.DAT` illustrates the layout; verify the
resource before using it. Dats-Overlay does not identify or convert resources.

Use a packaged launcher on a platform that applies overlays, and supply the
replacement DAT from your own local assets. This repository contains no retail
DAT payloads. Renaming a decoded PNG to `.DAT` does not make it a game resource.
Prepare or encode textures separately before installing the overlay.

1. Create `plugins/dats/dat-replacement-example/data/1C/59/00/` under the
   launcher's state root:
   - Portable layout: beside the executable, on Windows or in a tree without
     the package marker
   - Linux package or macOS app: under `~/.bahamut-launcher/`, or
     `$BAHAMUT_LAUNCHER_HOME` if it contains an absolute path

   See [Linux package layout](configuration.md#linux-package-layout) and
   [macOS app layout](configuration.md#macos-app-layout).
2. Copy the verified DAT into that folder as `CB.DAT`. Leave the file in the
   game installation unchanged.
3. Create `plugins/dats/dat-replacement-example/overlay.toml`:

   ```toml
   manifest_schema_version = 1
   id = "dat-replacement-example"
   name = "DAT Replacement Example"
   author = "Aeshur"
   version = "1.0.0"
   description = "Replaces a verified DAT resource at a game path."
   ```

   Use your own author and package name for your package. Author, version,
   and description are required even though the compact Dats-Overlay list
   does not show all of them.
4. Check the complete layout below. This example shows a portable installation.
   Avoid an extra enclosing folder or accidental `.toml.txt` or `.DAT.png`
   extensions.

   ```text
   <launcher-dir>/
     bahamut-launcher.exe
     plugins/
       dats/
         dat-replacement-example/
           overlay.toml
           data/
             1C/
               59/
                 00/
                   CB.DAT
   ```

5. Reopen the launcher. In Extensions, select Dats-Overlay and enable DAT
   Replacement Example. Move it earlier if another enabled package replaces
   the same path: the first matching file wins. Individual DAT packages are
   managed inside Dats-Overlay.
6. Start a new game session and check the replacement. Changing the selection
   during play does not update the running session.
7. Close the game, disable DAT Replacement Example, and launch again. The
   original resource should return; the package never edits the installed
   DAT tree.

## Creating another replacement

Find the resource's exact path relative to the game and mirror it inside your
package. Keep the original destination filename, even if the replacement comes
from a differently named resource. The payload must be compatible with that
resource type.

Labels such as `icon00083` or `icon00203` in a decoded preview collection are
lookup labels, not paths that Dats-Overlay can resolve. Confirm the label's
mapping to the actual DAT and the specific UI element before packaging it.
The example above is a layout guide, not a complete resource map.

Choose a unique lowercase package ID and use it as the directory name. A
package can contain multiple replacements, each at its own path relative to
the game. Use regular files and directories; linked folders and reparse points
are rejected. Keep notes and preview images outside the package payload tree.

## If the replacement does not appear

- Check the state root for the exact launcher you started. Separate portable
  launcher folders have separate packages and settings. Linux packages and
  macOS apps run by the same user share one state root.
- If the package is absent from Dats-Overlay, check the manifest, matching
  directory ID, and extra nesting. See
  [DAT troubleshooting](troubleshooting.md#dat-overlays).
- If it appears, check that it is enabled, review conflicting packages, and
  start a new game session after changing the selection.
- If the original resource still appears, verify the target UI element and
  DAT path. A missing replacement falls back to the original game file.
- If the replacement is broken, disable the package and check its DAT format
  and contents. Dats-Overlay resolves files; it cannot encode images or repair
  incompatible resources.
