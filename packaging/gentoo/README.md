# Gentoo package template

[Back to the documentation index](../../docs/README.md)

Use `bahamut-launcher-bin.ebuild` in a local overlay to install the prebuilt
Linux release as `games-util/bahamut-launcher-bin`. No official overlay
carries it. The template was tested in a Gentoo container with a locally
built tar.gz, not a published release download.

## Local overlay recipe

1. Create the overlay directory and copy the template. Name the ebuild with
   a released version; `<PV>` is the tag without its leading `v`:

   ```
   mkdir -p /var/db/repos/local/games-util/bahamut-launcher-bin
   cp bahamut-launcher-bin.ebuild \
      /var/db/repos/local/games-util/bahamut-launcher-bin/bahamut-launcher-bin-<PV>.ebuild
   cp metadata.xml /var/db/repos/local/games-util/bahamut-launcher-bin/
   ```

2. Generate the manifest to fetch the tar.gz and record its digests:

   ```
   cd /var/db/repos/local/games-util/bahamut-launcher-bin
   ebuild bahamut-launcher-bin-<PV>.ebuild manifest
   ```

3. For a new overlay, register it in `repos.conf` and `metadata/layout.conf`.
   Accept the `~amd64` keyword, then install:

   ```
   emerge --ask games-util/bahamut-launcher-bin
   ```

## License

The ebuild sets `LICENSE="MIT BSD-2 OFL-1.1"`. Review the MinGW-w64 runtime
notice in the tar.gz's `licenses/` directory before publishing the ebuild;
it carries its own terms.

## Versions

`SRC_URI` resolves to
`releases/download/v${PV}/bahamut-launcher-v${PV}-linux-x86_64.tar.gz`.
Use only stable release tags. Prerelease suffixes are invalid Gentoo versions
and have no matching asset name.

## Wine

On x86_64, the launcher downloads Wine on the first Play and stores it in
`~/.bahamut-launcher/runtime`. The ebuild therefore has no `virtual/wine`
dependency. The `@system` set provides `tar` and `xz` to unpack the download.

To use another Wine, set `BAHAMUT_WINE` to its `wine` binary. It needs 32-bit
support: the `abi_x86_32` or `wow64` USE flag on wine-vanilla or wine-staging.
Use `eselect wine list` to see the slots.

## Layout

The ebuild installs the payload at `/opt/bahamut-launcher`, links
`/usr/bin/bahamut-launcher` to it, and installs the desktop entry and 48, 128,
and 256 pixel icons. The `.bahamut-launcher-package` marker keeps launcher
state in `~/.bahamut-launcher`.

Because the binary is prebuilt, the ebuild restricts mirror and strip and
sets `QA_PREBUILT`.
