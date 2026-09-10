// W9-off erases its arguments, clocks, counters and dispatch.
macro_rules! boundary_begin {
    ($profile:expr, $cost:ident) => {
        #[cfg(feature = "audit-boundary-test-profile")]
        if let Some(profile) = $profile.as_deref_mut() {
            profile.begin(lumin_model::audit_boundary_diagnostic::AuditBoundaryCost::$cost);
        }
    };
}
macro_rules! boundary_end {
    ($profile:expr, $cost:ident) => {
        #[cfg(feature = "audit-boundary-test-profile")]
        if let Some(profile) = $profile.as_deref_mut() {
            profile.end(lumin_model::audit_boundary_diagnostic::AuditBoundaryCost::$cost);
        }
    };
}
macro_rules! boundary_cost {
    ($profile:expr, $cost:ident, $expression:expr) => {{
        boundary_begin!($profile, $cost);
        let result = $expression;
        boundary_end!($profile, $cost);
        result
    }};
}
macro_rules! boundary_context {
    ($profile:ident, $context:ident, |$boundary:ident| $body:expr) => {{
        #[cfg(feature = "audit-boundary-test-profile")]
        let mut boundary_recorder = $profile.as_ref().map(|_| {
            crate::audit_lifecycle_profile::BoundaryProfiler::new(
                lumin_model::audit_boundary_diagnostic::AuditBoundaryContext::$context,
            )
        });
        #[cfg(feature = "audit-boundary-test-profile")]
        let $boundary = boundary_recorder.as_mut();
        let result = $body;
        #[cfg(feature = "audit-boundary-test-profile")]
        if let (Some(profile), Some(recorder)) = ($profile.as_deref_mut(), boundary_recorder) {
            profile.record_boundary(recorder.finish());
        }
        result
    }};
}
// Explicit local lending only; never stored on a product resource.
#[cfg(feature = "audit-lifecycle-test-profile")]
pub(crate) enum DatabaseObserver<'a> {
    Lifecycle(&'a mut crate::audit_lifecycle_profile::LifecycleProfiler),
    #[cfg(feature = "audit-boundary-test-profile")]
    Boundary(&'a mut crate::audit_lifecycle_profile::BoundaryProfiler),
}
#[cfg(feature = "audit-lifecycle-test-profile")]
impl DatabaseObserver<'_> {
    pub(crate) fn begin(
        &mut self,
        cost: lumin_model::audit_lifecycle_diagnostic::AuditLifecycleCost,
    ) {
        match self {
            Self::Lifecycle(profile) => profile.begin(cost),
            #[cfg(feature = "audit-boundary-test-profile")]
            Self::Boundary(profile) => match boundary_cost(cost) {
                Some(cost) => profile.begin(cost),
                None => profile.invalidate("non-database leaf lent to boundary observer"),
            },
        }
    }
    pub(crate) fn end(
        &mut self,
        cost: lumin_model::audit_lifecycle_diagnostic::AuditLifecycleCost,
    ) {
        match self {
            Self::Lifecycle(profile) => profile.end(cost),
            #[cfg(feature = "audit-boundary-test-profile")]
            Self::Boundary(profile) => match boundary_cost(cost) {
                Some(cost) => profile.end(cost),
                None => profile.invalidate("non-database leaf lent to boundary observer"),
            },
        }
    }
}
#[cfg(feature = "audit-boundary-test-profile")]
fn boundary_cost(
    cost: lumin_model::audit_lifecycle_diagnostic::AuditLifecycleCost,
) -> Option<lumin_model::audit_boundary_diagnostic::AuditBoundaryCost> {
    use lumin_model::audit_boundary_diagnostic::AuditBoundaryCost as B;
    use lumin_model::audit_lifecycle_diagnostic::AuditLifecycleCost as L;
    match cost {
        L::NamespaceValidation => Some(B::NamespaceValidation),
        L::StoreHandleOpen => Some(B::StoreHandleOpen),
        L::BackendOpen => Some(B::BackendOpen),
        L::StoreValidation => Some(B::StoreValidation),
        L::ReadAdmission => Some(B::ReadAdmission),
        L::WriteAdmission => Some(B::WriteAdmission),
        L::BackendCommit => Some(B::BackendCommit),
        L::BackendAbort => Some(B::BackendAbort),
        L::DatabaseExplicitDrop => Some(B::DatabaseExplicitDrop),
        L::DatabaseReturnTail => Some(B::DatabaseReturnTail),
        _ => None,
    }
}
