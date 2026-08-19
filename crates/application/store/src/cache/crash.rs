const CRASH_POINT_ENV: &str = "LUMIN_TEST_CACHE_CLEANUP_CRASH_POINT";
const CRASH_EXIT_CODE: i32 = 95;
const INVALID_SELECTOR_EXIT_CODE: i32 = 96;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CacheCleanupCrashPoint {
    AfterAuthorization,
    AfterRenameVisible(u64),
    AfterPhysicalDurability(u64),
    AfterRowValidation(u64),
    BeforeResultCommit,
}

impl CacheCleanupCrashPoint {
    fn label(self) -> String {
        match self {
            Self::AfterAuthorization => "after-authorization".to_owned(),
            Self::AfterRenameVisible(ordinal) => format!("after-rename-visible:{ordinal}"),
            Self::AfterPhysicalDurability(ordinal) => {
                format!("after-physical-durability:{ordinal}")
            }
            Self::AfterRowValidation(ordinal) => format!("after-row-validation:{ordinal}"),
            Self::BeforeResultCommit => "before-result-commit".to_owned(),
        }
    }
}

pub(super) fn hit(point: CacheCleanupCrashPoint) {
    let Ok(requested) = std::env::var(CRASH_POINT_ENV) else {
        return;
    };
    if !valid_selector(&requested) {
        eprintln!("unknown cache cleanup test crash point: {requested}");
        std::process::exit(INVALID_SELECTOR_EXIT_CODE);
    }
    if requested == point.label() {
        std::process::exit(CRASH_EXIT_CODE);
    }
}

fn valid_selector(value: &str) -> bool {
    matches!(value, "after-authorization" | "before-result-commit")
        || [
            "after-rename-visible:",
            "after-physical-durability:",
            "after-row-validation:",
        ]
        .iter()
        .any(|prefix| {
            value.strip_prefix(prefix).is_some_and(|ordinal| {
                !ordinal.is_empty() && ordinal.bytes().all(|byte| byte.is_ascii_digit())
            })
        })
}
