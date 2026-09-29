# Creating a DAT overlay

Dats-Overlay maps replacement DAT files to paths relative to the game. It never
changes the installed game files.

See the [documentation index](README.md) and
[package configuration rules](configuration.md#datsini) for manifest rules,
selection, and priority.

## Official overlay package

Every release archive includes the official `bahamut-dats-overlay` package.
It is always enabled and takes priority over custom packages for paths it
contains. Its current manifest has no retail DAT changes. [Platform support](extensions.md#platform-support)
lists the launches that apply packages.

On Windows, official package files are part of the signed launcher release
inventory. A launcher update replaces listed official files and preserves
custom overlay packages and unrelated player files. In the Linux package or
a portable Linux or macOS tree they change only when a newer archive is
extracted or installed. The macOS app reads them from
`Contents/Resources/plugins/dats/` and changes them only with a newer app. In
the macOS app and the Linux package, custom packages live under the state
root's `plugins/dats/`, and a custom package cannot replace the official one.
If the official package is missing or its manifest is malformed, the launcher
ignores that package and can still start Play and load valid custom packages.
Replacing official files uses the launcher release verification rules. There
is no separate overlay update, verification, or repair operation.

## Example: package a DAT replacement

Choose a path relative to the exact client and a compatible DAT payload for that
resource. The example path `data/1C/59/00/CB.DAT` shows the package layout.
Verify the resource before using it. The overlay does not identify or convert
resources.

Use a packaged launcher on a platform that applies packages and provide the
replacement DAT from your own local assets. This repository contains no retail
DAT payloads. Renaming a decoded PNG to `.DAT` does not create a game resource.
Preparing or encoding textures is separate from installing an overlay.

1. Under the launcher's state root, create
   `plugins/dats/dat-replacement-example/data/1C/59/00/`: beside the
   executable in the portable layout (Windows, or a tree without the package
   marker), or under `~/.bahamut-launcher/` (or `$BAHAMUT_LAUNCHER_HOME` when
   it holds an absolute path) for the Linux package and the macOS app. See
   [Linux package layout](configuration.md#linux-package-layout) and
   [macOS app layout](configuration.md#macos-app-layout).
2. Copy the DAT you verified for this resource into that folder and name the
   copy `CB.DAT`. Do not change the file in the game installation.
3. Create `plugins/dats/dat-replacement-example/overlay.toml` with this
   content:

   ```toml
   manifest_schema_version = 1
   id = "dat-replacement-example"
   name = "DAT Replacement Example"
   author = "Aeshur"
   version = "1.0.0"
   description = "Replaces a verified DAT resource at a game path."
   ```

   When creating your own package, use your own author and package name. The
   current manifest schema requires author, version, and description even
   though the compact Dats-Overlay list does not display all of them.
4. Check that the complete package looks like this, with no extra enclosing
   folder and no accidental `.toml.txt` or `.DAT.png` extensions:

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

5. Reopen the launcher after installing the package. In Extensions, select
   Dats-Overlay and enable DAT Replacement Example. Move it earlier if another
   enabled package replaces the same path. The first matching file wins.
   Individual DAT packages belong inside Dats-Overlay.
6. Start a new game session and check that the selected resource reflects your
   replacement. Changing the selection while the game runs does not update that
   session.
7. Close the game, disable DAT Replacement Example, and launch again. The
   original resource should return. The package never edits the installed DAT
   tree.

## Creating another replacement

Find the exact path relative to the game for the resource you want to replace,
then mirror it below your package directory. Keep the original destination filename
even when the replacement comes from a differently named resource. Use a
compatible DAT payload for that resource type.

Names such as `icon00083` or `icon00203` in a decoded preview collection are
lookup labels, not paths that Dats-Overlay resolves. Confirm their mapping to
the actual DAT and the particular UI element before building a package. The
example establishes the package layout, not a complete resource map.

Use a unique lowercase package ID and make the directory name match it. One
package can contain multiple replacements, each at its own path relative to the
game. Copy regular files and directories into the package. Linked folders and
reparse points are rejected. Keep notes and preview images outside the package
payload tree.

## If the replacement does not appear

- Check the package under the state root of the exact launcher installation
  you started. In the portable layout, a second launcher folder has its own
  packages and settings. Every Linux package and macOS app a user runs shares
  one state root.
- If the package is missing from Dats-Overlay, check the manifest, matching
  directory ID, and extra nesting. See [DAT troubleshooting](troubleshooting.md#dat-overlays).
- If the package appears, check its enabled state, conflicting packages, and
  whether you started a new game session after changing it.
- If the original resource remains, verify the target UI element and DAT path.
  A missing replacement falls through to the original game file.
- If the replacement is broken, disable the package and check the DAT's
  format and contents. The overlay resolves files. It does not encode images
  or repair incompatible resources.
