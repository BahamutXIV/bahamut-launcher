# Gentoo package template

[Back to the documentation index](../../docs/README.md)

`bahamut-launcher-bin.ebuild` installs the prebuilt Linux release archive as
`games-util/bahamut-launcher-bin`. It is a template for a local overlay; no
official overlay carries it. The template was exercised in a Gentoo container
against a locally built archive, not against a published release asset.

## Local overlay recipe

1. Create the overlay directory and copy the template, naming the ebuild with
   a released version (`<PV>` is the tag without the leading `v`):

   ```
   mkdir -p /var/db/repos/local/games-util/bahamut-launcher-bin
   cp bahamut-launcher-bin.ebuild \
      /var/db/repos/local/games-util/bahamut-launcher-bin/bahamut-launcher-bin-<PV>.ebuild
   cp metadata.xml /var/db/repos/local/games-util/bahamut-launcher-bin/
   ```

2. Generate the manifest, which fetches the archive and records its digests:

   ```
   cd /var/db/repos/local/games-util/bahamut-launcher-bin
   ebuild bahamut-launcher-bin-<PV>.ebuild manifest
   ```

3. Register the overlay with `repos.conf` and `metadata/layout.conf` if it is
   new, accept the `~amd64` keyword, and install:

   ```
   emerge --ask games-util/bahamut-launcher-bin
   ```

## License

The ebuild sets `LICENSE="MIT BSD-2 OFL-1.1"`. The MinGW-w64 runtime notice in
the archive's `licenses/` directory carries its own terms; review them before
publishing the ebuild.

## Versions

`SRC_URI` resolves to
`releases/download/v${PV}/bahamut-launcher-v${PV}-linux-x86_64.tar.gz`.
Use stable release tags only. Prerelease tags carry suffixes that are not
valid Gentoo versions and have no matching asset name.

## Wine

On x86_64 the launcher downloads its own Wine on the first Play and keeps
it in `~/.bahamut-launcher/runtime`, so the ebuild does not depend on
`virtual/wine`. The download is unpacked with `tar` and `xz`, which the
`@system` set provides. To use another Wine, set `BAHAMUT_WINE` to its `wine`
binary; that Wine needs 32-bit support (the `abi_x86_32` or `wow64` USE flag
on wine-vanilla or wine-staging), and `eselect wine list` shows the slots.

## Layout

The ebuild installs the payload to `/opt/bahamut-launcher`, links
`/usr/bin/bahamut-launcher` to it, and installs the desktop entry and the
48, 128, and 256 pixel icons. The `.bahamut-launcher-package` marker keeps
launcher state in `~/.bahamut-launcher`. The binary is prebuilt, so the
ebuild restricts mirror and strip and sets `QA_PREBUILT`.
