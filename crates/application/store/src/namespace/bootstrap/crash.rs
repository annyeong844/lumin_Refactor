#[cfg(all(feature = "namespace-test-crash", not(debug_assertions)))]
compile_error!("namespace-test-crash is restricted to debug test builds");

#[cfg(feature = "namespace-test-crash")]
const CRASH_POINT_ENV: &str = "LUMIN_TEST_NAMESPACE_BOOTSTRAP_CRASH_POINT";
#[cfg(feature = "namespace-test-crash")]
const CRASH_EXIT_CODE: i32 = 97;
#[cfg(feature = "namespace-test-crash")]
const INVALID_SELECTOR_EXIT_CODE: i32 = 98;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::namespace) enum BootstrapCrashPoint {
    BeforeStateDirectory,
    AfterStateDirectoryCreated,
    AfterStateDirectoryFlushed,
    AfterLifecycleLockCreated,
    AfterLifecycleLockAcquired,
    AfterGlobalBindingAllocated,
    AfterLifecycleLockHeaderFlushed,
    AfterAttemptsDirectoryCreated,
    AfterAttemptsAnchorCreated,
    AfterAttemptsBindingAllocated,
    AfterAttemptsAnchorFlushed,
    AfterAttemptsParentFlushed,
    AfterRunsDirectoryCreated,
    AfterRunsAnchorCreated,
    AfterRunsBindingAllocated,
    AfterRunsAnchorFlushed,
    AfterRunsParentFlushed,
    AfterTrashDirectoryCreated,
    AfterTrashAnchorCreated,
    AfterTrashBindingAllocated,
    AfterTrashAnchorFlushed,
    AfterTrashParentFlushed,
    AfterCacheDirectoryCreated,
    AfterCacheAnchorCreated,
    AfterCacheBindingAllocated,
    AfterCacheAnchorFlushed,
    AfterCacheParentFlushed,
    AfterCacheEvictionsDirectoryCreated,
    AfterCacheEvictionsAnchorCreated,
    AfterCacheEvictionsBindingAllocated,
    AfterCacheEvictionsAnchorFlushed,
    AfterCacheEvictionsParentFlushed,
    AfterTrashParentFlushedForCacheEvictions,
    AfterAllParentsFlushed,
    BeforeMarkerCandidate,
    AfterMarkerCandidateCreated,
    AfterMarkerCandidateFlushed,
    AfterMarkerPublished,
    AfterMarkerParentFlushed,
    BeforeStoreCreation,
    AfterStoreCreated,
    AfterStoreInitialized,
    AfterStoreParentFlushed,
    AfterCompleteValidation,
}

#[cfg(any(test, feature = "namespace-test-crash"))]
impl BootstrapCrashPoint {
    fn label(self) -> &'static str {
        match self {
            Self::BeforeStateDirectory => "before-state-directory",
            Self::AfterStateDirectoryCreated => "after-state-directory-created",
            Self::AfterStateDirectoryFlushed => "after-state-directory-flushed",
            Self::AfterLifecycleLockCreated => "after-lifecycle-lock-created",
            Self::AfterLifecycleLockAcquired => "after-lifecycle-lock-acquired",
            Self::AfterGlobalBindingAllocated => "after-global-binding-allocated",
            Self::AfterLifecycleLockHeaderFlushed => "after-lifecycle-lock-header-flushed",
            Self::AfterAttemptsDirectoryCreated => "after-attempts-directory-created",
            Self::AfterAttemptsAnchorCreated => "after-attempts-anchor-created",
            Self::AfterAttemptsBindingAllocated => "after-attempts-binding-allocated",
            Self::AfterAttemptsAnchorFlushed => "after-attempts-anchor-flushed",
            Self::AfterAttemptsParentFlushed => "after-attempts-parent-flushed",
            Self::AfterRunsDirectoryCreated => "after-runs-directory-created",
            Self::AfterRunsAnchorCreated => "after-runs-anchor-created",
            Self::AfterRunsBindingAllocated => "after-runs-binding-allocated",
            Self::AfterRunsAnchorFlushed => "after-runs-anchor-flushed",
            Self::AfterRunsParentFlushed => "after-runs-parent-flushed",
            Self::AfterTrashDirectoryCreated => "after-trash-directory-created",
            Self::AfterTrashAnchorCreated => "after-trash-anchor-created",
            Self::AfterTrashBindingAllocated => "after-trash-binding-allocated",
            Self::AfterTrashAnchorFlushed => "after-trash-anchor-flushed",
            Self::AfterTrashParentFlushed => "after-trash-parent-flushed",
            Self::AfterCacheDirectoryCreated => "after-cache-directory-created",
            Self::AfterCacheAnchorCreated => "after-cache-anchor-created",
            Self::AfterCacheBindingAllocated => "after-cache-binding-allocated",
            Self::AfterCacheAnchorFlushed => "after-cache-anchor-flushed",
            Self::AfterCacheParentFlushed => "after-cache-parent-flushed",
            Self::AfterCacheEvictionsDirectoryCreated => "after-cache-evictions-directory-created",
            Self::AfterCacheEvictionsAnchorCreated => "after-cache-evictions-anchor-created",
            Self::AfterCacheEvictionsBindingAllocated => "after-cache-evictions-binding-allocated",
            Self::AfterCacheEvictionsAnchorFlushed => "after-cache-evictions-anchor-flushed",
            Self::AfterCacheEvictionsParentFlushed => "after-cache-evictions-parent-flushed",
            Self::AfterTrashParentFlushedForCacheEvictions => {
                "after-trash-parent-flushed-for-cache-evictions"
            }
            Self::AfterAllParentsFlushed => "after-all-parents-flushed",
            Self::BeforeMarkerCandidate => "before-marker-candidate",
            Self::AfterMarkerCandidateCreated => "after-marker-candidate-created",
            Self::AfterMarkerCandidateFlushed => "after-marker-candidate-flushed",
            Self::AfterMarkerPublished => "after-marker-published",
            Self::AfterMarkerParentFlushed => "after-marker-parent-flushed",
            Self::BeforeStoreCreation => "before-store-creation",
            Self::AfterStoreCreated => "after-store-created",
            Self::AfterStoreInitialized => "after-store-initialized",
            Self::AfterStoreParentFlushed => "after-store-parent-flushed",
            Self::AfterCompleteValidation => "after-complete-validation",
        }
    }
}

pub(in crate::namespace) fn hit(point: BootstrapCrashPoint) {
    #[cfg(feature = "namespace-test-crash")]
    {
        let Ok(requested) = std::env::var(CRASH_POINT_ENV) else {
            return;
        };
        if !ALL_POINTS
            .iter()
            .any(|candidate| candidate.label() == requested)
        {
            eprintln!("unknown namespace bootstrap test crash point: {requested}");
            std::process::exit(INVALID_SELECTOR_EXIT_CODE);
        }
        if requested == point.label() {
            std::process::exit(CRASH_EXIT_CODE);
        }
    }
    #[cfg(not(feature = "namespace-test-crash"))]
    let _ = point;
}

#[cfg(any(test, feature = "namespace-test-crash"))]
const ALL_POINTS: [BootstrapCrashPoint; 44] = [
    BootstrapCrashPoint::BeforeStateDirectory,
    BootstrapCrashPoint::AfterStateDirectoryCreated,
    BootstrapCrashPoint::AfterStateDirectoryFlushed,
    BootstrapCrashPoint::AfterLifecycleLockCreated,
    BootstrapCrashPoint::AfterLifecycleLockAcquired,
    BootstrapCrashPoint::AfterGlobalBindingAllocated,
    BootstrapCrashPoint::AfterLifecycleLockHeaderFlushed,
    BootstrapCrashPoint::AfterAttemptsDirectoryCreated,
    BootstrapCrashPoint::AfterAttemptsAnchorCreated,
    BootstrapCrashPoint::AfterAttemptsBindingAllocated,
    BootstrapCrashPoint::AfterAttemptsAnchorFlushed,
    BootstrapCrashPoint::AfterAttemptsParentFlushed,
    BootstrapCrashPoint::AfterRunsDirectoryCreated,
    BootstrapCrashPoint::AfterRunsAnchorCreated,
    BootstrapCrashPoint::AfterRunsBindingAllocated,
    BootstrapCrashPoint::AfterRunsAnchorFlushed,
    BootstrapCrashPoint::AfterRunsParentFlushed,
    BootstrapCrashPoint::AfterTrashDirectoryCreated,
    BootstrapCrashPoint::AfterTrashAnchorCreated,
    BootstrapCrashPoint::AfterTrashBindingAllocated,
    BootstrapCrashPoint::AfterTrashAnchorFlushed,
    BootstrapCrashPoint::AfterTrashParentFlushed,
    BootstrapCrashPoint::AfterCacheDirectoryCreated,
    BootstrapCrashPoint::AfterCacheAnchorCreated,
    BootstrapCrashPoint::AfterCacheBindingAllocated,
    BootstrapCrashPoint::AfterCacheAnchorFlushed,
    BootstrapCrashPoint::AfterCacheParentFlushed,
    BootstrapCrashPoint::AfterCacheEvictionsDirectoryCreated,
    BootstrapCrashPoint::AfterCacheEvictionsAnchorCreated,
    BootstrapCrashPoint::AfterCacheEvictionsBindingAllocated,
    BootstrapCrashPoint::AfterCacheEvictionsAnchorFlushed,
    BootstrapCrashPoint::AfterCacheEvictionsParentFlushed,
    BootstrapCrashPoint::AfterTrashParentFlushedForCacheEvictions,
    BootstrapCrashPoint::AfterAllParentsFlushed,
    BootstrapCrashPoint::BeforeMarkerCandidate,
    BootstrapCrashPoint::AfterMarkerCandidateCreated,
    BootstrapCrashPoint::AfterMarkerCandidateFlushed,
    BootstrapCrashPoint::AfterMarkerPublished,
    BootstrapCrashPoint::AfterMarkerParentFlushed,
    BootstrapCrashPoint::BeforeStoreCreation,
    BootstrapCrashPoint::AfterStoreCreated,
    BootstrapCrashPoint::AfterStoreInitialized,
    BootstrapCrashPoint::AfterStoreParentFlushed,
    BootstrapCrashPoint::AfterCompleteValidation,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reviewed_bootstrap_crash_point_inventory_is_closed() {
        let labels = ALL_POINTS
            .iter()
            .map(|point| point.label())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ALL_POINTS.len(), 44);
        assert_eq!(labels.len(), ALL_POINTS.len());
    }
}
