//! Best-effort pre-launch controller step: log pad state and, when enabled, write a matching
//! retail `config.pad` selection from the controller GUIDs Wine has cached.

use std::fs;
use std::io;
use std::path::Path;

use super::wine_input_cache::{CachedPad, USER_REG_FILE, parse_directinput_cache};
use crate::config::retail_pad::{
    PAD_FILE_NAME, PadCandidate, PadPlan, PadSkip, PadWriteKind, plan_pad_update, read_pad_file,
    write_planned_image,
};
use crate::diagnostics::{InputDeviceEntry, pad_file_lines, parse_proc_input_devices};

/// Steam Input's emulated Xbox 360 pad: (vendor, product).
const XINPUT_PAD: (u16, u16) = (0x045E, 0x028E);

/// What [`prepare`] did; the launch ignores it because every outcome is already logged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PadSetupOutcome {
    Disabled,
    Unchanged,
    Written {
        kind: PadWriteKind,
        map_written: bool,
    },
    Skipped(PadSkip),
    Failed(String),
}

/// Cached pads, in cache order, whose device is currently attached.
///
/// Wine keeps cache entries for controllers that are gone, so entries without a present device
/// are stale and excluded.
pub(crate) fn candidates(cached: &[CachedPad], present: &[InputDeviceEntry]) -> Vec<PadCandidate> {
    cached
        .iter()
        .filter(|pad| {
            present.iter().any(|device| {
                device.vendor == pad.vendor
                    && device.product == pad.product
                    && device.could_be_gamepad()
            })
        })
        .map(|pad| PadCandidate {
            vendor: pad.vendor,
            product: pad.product,
            index: pad.index,
            instance: pad.instance,
            // Other pads may take the game's DirectInput route, where the XInput preset map
            // numbering is wrong, so they get the identity only.
            xinput_route: (pad.vendor, pad.product) == XINPUT_PAD,
        })
        .collect()
}

/// Logs pad state and, when `enabled`, writes the retail pad selection. Never fails the launch.
pub(crate) fn prepare(
    prefix: &Path,
    user_dir: &Path,
    proc_input_devices: Option<&str>,
    enabled: bool,
) -> PadSetupOutcome {
    let cached = read_cache(prefix);
    let path = user_dir.join(PAD_FILE_NAME);
    let existing = read_pad_file(&path);
    let read = match &existing {
        Ok(bytes) => Ok(bytes.as_deref()),
        Err(kind) => Err(*kind),
    };
    for line in pad_file_lines(read, Some(cached.as_slice())) {
        tracing::info!("Controller pad = {line}");
    }

    let present = match proc_input_devices {
        Some(text) => parse_proc_input_devices(text),
        None => {
            tracing::info!("Controller pad = host input list unavailable");
            Vec::new()
        }
    };
    let found = candidates(&cached, &present);
    tracing::info!("Controller pad candidates = {}", candidates_summary(&found));

    if !enabled {
        tracing::info!("Controller pad setup = off");
        return PadSetupOutcome::Disabled;
    }
    let existing = match existing {
        Ok(bytes) => bytes,
        Err(kind) => {
            tracing::warn!("Controller pad setup = skipped (config.pad unreadable)");
            return PadSetupOutcome::Failed(format!("config.pad unreadable ({kind:?})"));
        }
    };

    match plan_pad_update(existing.as_deref(), &found) {
        PadPlan::Unchanged { matched } => {
            tracing::info!(
                "Controller pad setup = unchanged (selection matches {})",
                pad_label(&matched)
            );
            PadSetupOutcome::Unchanged
        }
        PadPlan::Skip(skip) => {
            match skip {
                PadSkip::NoCandidates => tracing::info!(
                    "Controller pad setup = skipped (no present controller in the Wine cache; \
                     Wine records a controller after it first sees one, so a later launch can \
                     select it)"
                ),
                PadSkip::Ambiguous => tracing::info!(
                    "Controller pad setup = skipped (several cached controllers are present; \
                     selection left unchanged)"
                ),
            }
            PadSetupOutcome::Skipped(skip)
        }
        PadPlan::Write {
            image,
            chosen,
            kind,
            map_written,
        } => match write_planned_image(&path, existing.as_deref(), &image) {
            Ok(()) => {
                let kind_text = match kind {
                    PadWriteKind::Created => "created",
                    PadWriteKind::IdentityRepaired => "identity repaired",
                };
                let map_text = if map_written {
                    "XInput preset map written"
                } else {
                    "map unchanged"
                };
                let backup = if existing.is_some() {
                    "; previous file saved as config.pad.bak"
                } else {
                    ""
                };
                tracing::info!(
                    "Controller pad setup = wrote config.pad ({kind_text}, {map_text}) for {}{backup}",
                    pad_label(&chosen)
                );
                PadSetupOutcome::Written { kind, map_written }
            }
            Err(error) => {
                let kind = error.kind();
                tracing::warn!("Controller pad setup = write failed ({kind:?})");
                PadSetupOutcome::Failed(format!("write failed ({kind:?})"))
            }
        },
    }
}

fn read_cache(prefix: &Path) -> Vec<CachedPad> {
    match fs::read_to_string(prefix.join(USER_REG_FILE)) {
        Ok(text) => parse_directinput_cache(&text),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(error) => {
            tracing::warn!(
                "Controller pad = Wine registry unreadable ({:?})",
                error.kind()
            );
            Vec::new()
        }
    }
}

fn pad_label(pad: &PadCandidate) -> String {
    format!("{:04x}:{:04x} idx {}", pad.vendor, pad.product, pad.index)
}

fn candidates_summary(candidates: &[PadCandidate]) -> String {
    if candidates.is_empty() {
        return "0".into();
    }
    let items: Vec<String> = candidates
        .iter()
        .map(|pad| {
            let route = if pad.xinput_route {
                "xinput"
            } else {
                "directinput"
            };
            format!("{} {route}", pad_label(pad))
        })
        .collect();
    format!("{} ({})", candidates.len(), items.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const XBOX_GUID: [u8; 16] = [
        0xd2, 0xf6, 0x48, 0xbb, 0x1b, 0xc3, 0xf1, 0x11, 0x80, 0x01, 0x44, 0x45, 0x53, 0x54, 0x00,
        0x00,
    ];
    const DECK_GUID: [u8; 16] = [
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10,
    ];
    const OTHER_GUID: [u8; 16] = [0xaa; 16];
    const XBOX_PRODUCT_BYTES: [u8; 16] = [
        0x5e, 0x04, 0x8e, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x50, 0x49, 0x44, 0x56, 0x49,
        0x44,
    ];
    const REG_HEADER: &str =
        "WINE REGISTRY Version 2\n;; All keys relative to \\\\User\\\\S-1-5-21-0-0-0-1000\n\n";
    const XBOX_PROC: &str = "I: Bus=0003 Vendor=045e Product=028e Version=0110\n\
                             N: Name=\"Microsoft X-Box 360 pad 0\"\n\
                             H: Handlers=event20 js0\n\n";
    const KEYBOARD_PROC: &str = "I: Bus=0011 Vendor=0001 Product=0001 Version=ab41\n\
                                 N: Name=\"AT keyboard\"\n\
                                 H: Handlers=sysrq kbd event3\n\n";

    fn reg_key(vid_pid: &str, guid: [u8; 16]) -> String {
        let hex = guid
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<Vec<_>>()
            .join(",");
        format!(
            "[System\\\\CurrentControlSet\\\\Control\\\\MediaProperties\\\\PrivateProperties\\\\\
             DirectInput\\\\{vid_pid}\\\\Calibration\\\\0] 1791465929\n\
             #time=1dd57287d06dab6\n\
             \"GUID\"=hex:{hex}\n\
             \"Joystick Id\"=hex:00,00,00,00\n\n"
        )
    }

    fn user_reg(keys: &[(&str, [u8; 16])]) -> String {
        let mut text = REG_HEADER.to_string();
        for (vid_pid, guid) in keys {
            text.push_str(&reg_key(vid_pid, *guid));
        }
        text
    }

    fn proc_pad(vendor: u16, product: u16, handlers: &str) -> String {
        format!(
            "I: Bus=0003 Vendor={vendor:04x} Product={product:04x} Version=0110\n\
             N: Name=\"pad\"\n\
             H: Handlers={handlers}\n\n"
        )
    }

    struct Fixture {
        _root: tempfile::TempDir,
        prefix: PathBuf,
        user: PathBuf,
    }

    fn fixture(user_reg: Option<&str>) -> Fixture {
        let root = tempfile::tempdir().unwrap();
        let prefix = root.path().join("prefix");
        let user = root.path().join("user");
        fs::create_dir(&prefix).unwrap();
        fs::create_dir(&user).unwrap();
        if let Some(text) = user_reg {
            fs::write(prefix.join(USER_REG_FILE), text).unwrap();
        }
        Fixture {
            _root: root,
            prefix,
            user,
        }
    }

    fn run(f: &Fixture, proc_text: Option<&str>, enabled: bool) -> PadSetupOutcome {
        prepare(&f.prefix, &f.user, proc_text, enabled)
    }

    fn listing(f: &Fixture) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(&f.user)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn pad_path(f: &Fixture) -> PathBuf {
        f.user.join(PAD_FILE_NAME)
    }

    #[test]
    fn enabled_launch_creates_selection_then_leaves_it_unchanged() {
        let f = fixture(Some(&user_reg(&[("VID_045E&PID_028E", XBOX_GUID)])));

        assert_eq!(
            run(&f, Some(XBOX_PROC), true),
            PadSetupOutcome::Written {
                kind: PadWriteKind::Created,
                map_written: true
            }
        );
        let bytes = fs::read(pad_path(&f)).unwrap();
        assert_eq!(bytes.len(), 328);
        assert_eq!(&bytes[0x04..0x14], &XBOX_GUID[..]);
        assert_eq!(&bytes[0x14..0x24], &XBOX_PRODUCT_BYTES[..]);
        assert_eq!(listing(&f), ["config.pad"]);

        assert_eq!(run(&f, Some(XBOX_PROC), true), PadSetupOutcome::Unchanged);
        assert_eq!(fs::read(pad_path(&f)).unwrap(), bytes);
        assert_eq!(listing(&f), ["config.pad"]);
    }

    #[test]
    fn disabled_launch_writes_nothing() {
        let f = fixture(Some(&user_reg(&[("VID_045E&PID_028E", XBOX_GUID)])));

        assert_eq!(run(&f, Some(XBOX_PROC), false), PadSetupOutcome::Disabled);
        assert!(listing(&f).is_empty());
    }

    #[test]
    fn absent_cached_pad_is_skipped() {
        let f = fixture(Some(&user_reg(&[("VID_045E&PID_028E", XBOX_GUID)])));

        assert_eq!(
            run(&f, Some(KEYBOARD_PROC), true),
            PadSetupOutcome::Skipped(PadSkip::NoCandidates)
        );
        assert!(listing(&f).is_empty());
    }

    #[test]
    fn unavailable_host_input_list_is_skipped() {
        let f = fixture(Some(&user_reg(&[("VID_045E&PID_028E", XBOX_GUID)])));

        assert_eq!(
            run(&f, None, true),
            PadSetupOutcome::Skipped(PadSkip::NoCandidates)
        );
        assert!(listing(&f).is_empty());
    }

    #[test]
    fn several_present_non_preferred_pads_are_ambiguous() {
        let f = fixture(Some(&user_reg(&[
            ("VID_28DE&PID_1205", DECK_GUID),
            ("VID_054C&PID_09CC", OTHER_GUID),
        ])));
        let proc_text = proc_pad(0x28de, 0x1205, "event5 js0") + &proc_pad(0x054c, 0x09cc, "js1");

        assert_eq!(
            run(&f, Some(&proc_text), true),
            PadSetupOutcome::Skipped(PadSkip::Ambiguous)
        );
        assert!(listing(&f).is_empty());
    }

    #[test]
    fn stale_valid_file_is_repaired_with_backup() {
        let f = fixture(Some(&user_reg(&[("VID_045E&PID_028E", XBOX_GUID)])));
        let mut stale = vec![0u8; 328];
        stale[..4].copy_from_slice(&[0x11, 0x02, 0x10, 0x20]);
        stale[4..20].copy_from_slice(&OTHER_GUID);
        fs::write(pad_path(&f), &stale).unwrap();

        let outcome = run(&f, Some(XBOX_PROC), true);

        assert!(matches!(
            outcome,
            PadSetupOutcome::Written {
                kind: PadWriteKind::IdentityRepaired,
                ..
            }
        ));
        assert_eq!(fs::read(f.user.join("config.pad.bak")).unwrap(), stale);
        let bytes = fs::read(pad_path(&f)).unwrap();
        assert_eq!(&bytes[0x04..0x14], &XBOX_GUID[..]);
        assert_eq!(listing(&f), ["config.pad", "config.pad.bak"]);
    }

    #[test]
    fn unreadable_wine_registry_is_treated_as_empty_cache() {
        let f = fixture(None);
        fs::create_dir(f.prefix.join(USER_REG_FILE)).unwrap();

        assert_eq!(
            run(&f, Some(XBOX_PROC), true),
            PadSetupOutcome::Skipped(PadSkip::NoCandidates)
        );
        assert!(listing(&f).is_empty());
    }

    #[test]
    fn unreadable_pad_file_fails_without_writing() {
        let f = fixture(Some(&user_reg(&[("VID_045E&PID_028E", XBOX_GUID)])));
        fs::create_dir(pad_path(&f)).unwrap();

        assert!(matches!(
            run(&f, Some(XBOX_PROC), true),
            PadSetupOutcome::Failed(_)
        ));
        assert!(pad_path(&f).is_dir());
        assert_eq!(listing(&f), ["config.pad"]);
    }

    #[test]
    fn candidates_route_only_the_steam_input_xbox_pad_through_xinput() {
        let cached = [
            CachedPad {
                vendor: 0x045e,
                product: 0x028e,
                index: 0,
                instance: XBOX_GUID,
            },
            CachedPad {
                vendor: 0x28de,
                product: 0x1205,
                index: 1,
                instance: DECK_GUID,
            },
            CachedPad {
                vendor: 0x054c,
                product: 0x09cc,
                index: 2,
                instance: OTHER_GUID,
            },
        ];
        let present = parse_proc_input_devices(&(proc_pad(0x28de, 0x1205, "js1") + XBOX_PROC));

        let found = candidates(&cached, &present);

        assert_eq!(
            found
                .iter()
                .map(|pad| (pad.vendor, pad.product, pad.index, pad.xinput_route))
                .collect::<Vec<_>>(),
            [(0x045e, 0x028e, 0, true), (0x28de, 0x1205, 1, false)]
        );
        assert_eq!(found[0].instance, XBOX_GUID);
    }

    #[test]
    fn candidate_summary_lists_route_and_never_guids() {
        let cached = [CachedPad {
            vendor: 0x045e,
            product: 0x028e,
            index: 0,
            instance: XBOX_GUID,
        }];
        let present = parse_proc_input_devices(XBOX_PROC);

        assert_eq!(candidates_summary(&[]), "0");
        assert_eq!(
            candidates_summary(&candidates(&cached, &present)),
            "1 (045e:028e idx 0 xinput)"
        );
    }
}
