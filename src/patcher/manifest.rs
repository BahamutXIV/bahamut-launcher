// Bahamut Launcher - companion launcher for the BahamutXIV FFXIV 1.23b
// preservation server.
// Copyright (c) 2026 Aeshur
// Licensed under the MIT License; see LICENSE.md for the full text.
//
// SPDX-License-Identifier: MIT

use std::collections::HashMap;
use std::sync::LazyLock;

/// Patch path, size, and CRC32; SHA-256 identities come from the public manifest.
#[derive(Debug, Clone, Copy)]
pub struct PatchEntry {
    pub path: &'static str,
    pub size: u64,
    pub crc32: u32,
}

const fn entry(path: &'static str, size: u64, crc32: u32) -> PatchEntry {
    PatchEntry { path, size, crc32 }
}

/// Complete 1.x patch chain, oldest first; application uses filename order.
#[rustfmt::skip]
pub const PATCH_MANIFEST: &[PatchEntry] = &[
    entry("2d2a390f/patch/D2010.09.18.0000.patch", 0x0055_0467,  0x47DDE5ED),

    entry("48eca647/patch/D2010.09.19.0000.patch", 444_398_866,  0xD55C7ACD),
    entry("48eca647/patch/D2010.09.23.0000.patch", 6_907_277,    0xCA135D55),
    entry("48eca647/patch/D2010.09.28.0000.patch", 18_803_280,   0xB19B32FE),

    entry("48eca647/patch/D2010.10.07.0001.patch", 19_226_330,   0xD6118CEE),
    entry("48eca647/patch/D2010.10.14.0000.patch", 19_464_329,   0x34BF6A99),
    entry("48eca647/patch/D2010.10.22.0000.patch", 19_778_252,   0x2543DB5C),
    entry("48eca647/patch/D2010.10.26.0000.patch", 19_778_391,   0x20F94876),

    entry("48eca647/patch/D2010.11.25.0002.patch", 250_718_651,  0x5FBB5B24),
    entry("48eca647/patch/D2010.11.30.0000.patch", 6_921_623,    0xA5479111),

    entry("48eca647/patch/D2010.12.06.0000.patch", 7_158_904,    0xCAD6BC31),
    entry("48eca647/patch/D2010.12.13.0000.patch", 263_311_481,  0xE51EFC06),
    entry("48eca647/patch/D2010.12.21.0000.patch", 7_521_358,    0x93EE1510),

    entry("48eca647/patch/D2011.01.18.0000.patch", 9_954_265,    0x059E8900),

    entry("48eca647/patch/D2011.02.01.0000.patch", 11_632_816,   0x9EE60B39),
    entry("48eca647/patch/D2011.02.10.0000.patch", 11_714_096,   0x0ADE7243),

    entry("48eca647/patch/D2011.03.01.0000.patch", 77_464_101,   0x7818B5BF),
    entry("48eca647/patch/D2011.03.24.0000.patch", 108_923_937,  0xF21852AD),
    entry("48eca647/patch/D2011.03.30.0000.patch", 109_010_880,  0x84CB2682),

    entry("48eca647/patch/D2011.04.13.0000.patch", 341_603_850,  0xFF6C3DB0),
    entry("48eca647/patch/D2011.04.21.0000.patch", 343_579_198,  0x57F4041C),

    entry("48eca647/patch/D2011.05.19.0000.patch", 344_239_925,  0xB16FF18C),

    entry("48eca647/patch/D2011.06.10.0000.patch", 344_334_860,  0xB1CAA88B),

    entry("48eca647/patch/D2011.07.20.0000.patch", 584_926_805,  0x2EA149A9),
    entry("48eca647/patch/D2011.07.26.0000.patch", 7_649_141,    0x5670BA07),

    entry("48eca647/patch/D2011.08.05.0000.patch", 152_064_532,  0x0D9E9FD8),
    entry("48eca647/patch/D2011.08.09.0000.patch", 8_573_687,    0x9B54551A),
    entry("48eca647/patch/D2011.08.16.0000.patch", 6_118_907,    0x75231C57),

    entry("48eca647/patch/D2011.10.04.0000.patch", 677_633_296,  0x95C15318),
    entry("48eca647/patch/D2011.10.12.0001.patch", 28_941_655,   0xB37993E3),
    entry("48eca647/patch/D2011.10.27.0000.patch", 29_179_764,   0x977480DC),

    entry("48eca647/patch/D2011.12.14.0000.patch", 374_617_428,  0xC6FE8FED),
    entry("48eca647/patch/D2011.12.23.0000.patch", 22_363_713,   0x93137C93),

    entry("48eca647/patch/D2012.01.18.0000.patch", 48_998_794,   0x9E55EC7E),
    entry("48eca647/patch/D2012.01.24.0000.patch", 49_126_606,   0x3008D942),
    entry("48eca647/patch/D2012.01.31.0000.patch", 49_536_396,   0x60FDBD0B),

    entry("48eca647/patch/D2012.03.07.0000.patch", 320_630_782,  0x885AD768),
    entry("48eca647/patch/D2012.03.09.0000.patch", 8_312_819,    0xC0040D8C),
    entry("48eca647/patch/D2012.03.22.0000.patch", 22_027_738,   0xEABC501B),
    entry("48eca647/patch/D2012.03.29.0000.patch", 8_322_920,    0x63811C35),

    entry("48eca647/patch/D2012.04.04.0000.patch", 8_678_570,    0xF6E43EEC),
    entry("48eca647/patch/D2012.04.23.0001.patch", 289_511_791,  0x6C3C0201),

    entry("48eca647/patch/D2012.05.08.0000.patch", 27_266_546,   0xB6AABF18),
    entry("48eca647/patch/D2012.05.15.0000.patch", 27_416_023,   0x2D428126),
    entry("48eca647/patch/D2012.05.22.0000.patch", 27_742_726,   0x9163549D),

    entry("48eca647/patch/D2012.06.06.0000.patch", 129_984_024,  0x21DF7238),
    entry("48eca647/patch/D2012.06.19.0000.patch", 133_434_217,  0x8280988A),
    entry("48eca647/patch/D2012.06.26.0000.patch", 133_581_048,  0x4CF33FC8),

    entry("48eca647/patch/D2012.07.21.0000.patch", 253_224_781,  0xA8A42A32),

    entry("48eca647/patch/D2012.08.10.0000.patch", 42_851_112,   0xD8ED4CE3),

    entry("48eca647/patch/D2012.09.06.0000.patch", 20_566_711,   0x4235DF72),
    entry("48eca647/patch/D2012.09.19.0001.patch", 20_874_726,   0x8A775526),
];

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicPatchIdentities {
    files: Vec<PublicPatchIdentity>,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicPatchIdentity {
    runtime_path: String,
    sha256: String,
}

static SHA256_BY_RUNTIME_PATH: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    let manifest: PublicPatchIdentities =
        serde_json::from_str(include_str!("../../manifests/patches-1.23b.json"))
            .expect("the public patch manifest must be valid JSON");
    manifest
        .files
        .into_iter()
        .map(|file| (file.runtime_path, file.sha256))
        .collect()
});

/// Resolve the canonical SHA-256 identity by runtime patch path.
pub(crate) fn expected_sha256(runtime_path: &str) -> Option<&'static str> {
    SHA256_BY_RUNTIME_PATH.get(runtime_path).map(String::as_str)
}

/// Total bytes across the manifest, used for progress totals.
pub fn total_bytes() -> u64 {
    PATCH_MANIFEST.iter().map(|e| e.size).sum()
}

/// Return the `/`-separated manifest leaf used for plan and worker matching.
pub(crate) fn leaf_of(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PublicPatchManifest {
        schema_version: u32,
        repository: String,
        commit: String,
        target_client_version: String,
        archive_name: String,
        files: Vec<PublicPatchFile>,
    }

    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct PublicPatchFile {
        path: String,
        runtime_path: String,
        size: u64,
        crc32: String,
        sha256: String,
    }

    #[test]
    fn every_key_lives_under_the_patch_subpath() {
        for e in PATCH_MANIFEST {
            assert!(e.path.contains("/patch/"), "unexpected key: {}", e.path);
            assert!(e.path.ends_with(".patch"), "unexpected key: {}", e.path);
        }
    }

    #[test]
    fn total_is_in_the_expected_six_gig_range() {
        // The entries total roughly 6.3 GB; allow corrected manifest values
        // without weakening the expected order of magnitude.
        let total = total_bytes();
        assert!(
            (6_000_000_000..7_000_000_000).contains(&total),
            "got {total}"
        );
    }

    #[test]
    fn public_patch_manifest_matches_runtime_manifest() {
        let public: PublicPatchManifest =
            serde_json::from_str(include_str!("../../manifests/patches-1.23b.json")).unwrap();

        assert_eq!(public.schema_version, 1);
        assert_eq!(public.repository, "BahamutXIV/bahamut-private-assets");
        assert_eq!(public.commit.len(), 40);
        assert!(
            public
                .commit
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_eq!(
            public.target_client_version,
            crate::version::FFXIV_GAME_VERSION
        );
        assert_eq!(public.archive_name, crate::patcher::extract::PATCH_ZIP_NAME);
        assert_eq!(public.files.len(), PATCH_MANIFEST.len());

        for (public_file, runtime_file) in public.files.iter().zip(PATCH_MANIFEST) {
            assert_eq!(public_file.runtime_path, runtime_file.path);
            assert_eq!(
                public_file.path,
                format!("patches-1.23b/{}", runtime_file.path)
            );
            assert_eq!(public_file.size, runtime_file.size);
            assert_eq!(public_file.crc32, format!("{:08X}", runtime_file.crc32));
            assert_eq!(public_file.sha256.len(), 64);
            assert!(
                public_file
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            );
            assert_eq!(
                expected_sha256(runtime_file.path),
                Some(public_file.sha256.as_str())
            );
        }
    }
}
