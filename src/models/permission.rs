// src/models/permission.rs
//
// Scope-based permission model (#331). Scopes are the atomic primitive:
// every capability is a `resource:action` string. Roles are named scope
// bundles; per-user `extra_scopes` union onto the role bundle. The matcher
// understands the global `*` superuser token and per-resource `resource:*`
// wildcards.
//
// This module is pure: no DB, no HTTP. Enforcement (middleware, guards) lands
// in a later chunk.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Owner,
    FleetManager,
    #[default]
    Dispatcher,
}

impl Role {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::FleetManager => "fleet_manager",
            Self::Dispatcher => "dispatcher",
        }
    }
}

impl std::str::FromStr for Role {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "owner" => Ok(Self::Owner),
            "fleet_manager" => Ok(Self::FleetManager),
            "dispatcher" => Ok(Self::Dispatcher),
            other => Err(format!("unknown role: {other}")),
        }
    }
}

/// Global superuser scope — grants every capability.
pub const SCOPE_SUPERUSER: &str = "*";

/// The dispatcher operational bundle (per the #331 design). No `users:*`,
/// no `loads:settle`/`loads:invoice`, no master-data deletes.
pub const DISPATCHER_SCOPES: &[&str] = &[
    "loads:read",
    "loads:write",
    "trips:read",
    "trips:write",
    "trips:delete",
    "drivers:read",
    "drivers:write",
    "trucks:read",
    "trucks:write",
    "trailers:read",
    "trailers:write",
    "maintenance:read",
    "maintenance:write",
    "facilities:read",
    "facilities:write",
    "terminals:read",
    "blobs:read",
    "blobs:write",
    "events:read",
    "api_keys:read",
    "api_keys:write",
    "api_keys:delete",
];

/// Canonical scope vocabulary — every `resource:action` string the system
/// recognizes, plus the elevated verbs and the global `*`. Useful for
/// validation (e.g. rejecting unknown `extra_scopes`) and documentation.
pub const ALL_SCOPES: &[&str] = &[
    "loads:read",
    "loads:write",
    "loads:delete",
    "loads:settle",
    "loads:invoice",
    "trips:read",
    "trips:write",
    "trips:delete",
    "drivers:read",
    "drivers:write",
    "drivers:delete",
    "trucks:read",
    "trucks:write",
    "trucks:delete",
    "trailers:read",
    "trailers:write",
    "trailers:delete",
    "maintenance:read",
    "maintenance:write",
    "maintenance:delete",
    "facilities:read",
    "facilities:write",
    "facilities:delete",
    "terminals:read",
    "terminals:write",
    "terminals:delete",
    "blobs:read",
    "blobs:write",
    "blobs:delete",
    "events:read",
    "users:read",
    "users:write",
    "users:delete",
    "api_keys:read",
    "api_keys:write",
    "api_keys:delete",
    SCOPE_SUPERUSER,
];

/// The static scope bundle for a role.
///
/// `Owner` and `FleetManager` are operationally identical (`["*"]`); the
/// distinction between them is enforced by owner-protection rules elsewhere,
/// not by scopes.
pub fn role_scopes(role: Role) -> Vec<String> {
    match role {
        Role::Owner | Role::FleetManager => vec![SCOPE_SUPERUSER.to_string()],
        Role::Dispatcher => DISPATCHER_SCOPES.iter().map(|s| s.to_string()).collect(),
    }
}

/// Effective scopes = role bundle ∪ extra grants.
pub fn effective_scopes(role: Role, extra: &[String]) -> Vec<String> {
    let mut out = role_scopes(role);
    for s in extra {
        if !out.iter().any(|e| e == s) {
            out.push(s.clone());
        }
    }
    out
}

/// True if `required` is granted by `effective`. A required scope `r:a` is
/// satisfied by an exact match, by the per-resource wildcard `r:*`, or by the
/// global superuser `*`.
pub fn scope_granted(effective: &[String], required: &str) -> bool {
    let resource_wildcard = required
        .split_once(':')
        .map(|(resource, _action)| format!("{resource}:*"));

    effective.iter().any(|s| {
        s == SCOPE_SUPERUSER
            || s == required
            || resource_wildcard.as_deref() == Some(s.as_str())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_role_roundtrip() {
        for r in ["owner", "fleet_manager", "dispatcher"] {
            let role: Role = r.parse().unwrap();
            assert_eq!(role.as_str(), r);
        }
    }

    #[test]
    fn test_role_unknown() {
        assert!("admin".parse::<Role>().is_err());
    }

    #[test]
    fn test_role_default_is_dispatcher() {
        assert_eq!(Role::default(), Role::Dispatcher);
    }

    #[test]
    fn test_role_serde_snake_case() {
        assert_eq!(serde_json::to_string(&Role::FleetManager).unwrap(), "\"fleet_manager\"");
        let r: Role = serde_json::from_str("\"fleet_manager\"").unwrap();
        assert_eq!(r, Role::FleetManager);
    }

    #[test]
    fn test_owner_and_fleet_manager_pass_everything() {
        for role in [Role::Owner, Role::FleetManager] {
            let eff = effective_scopes(role, &[]);
            for scope in ALL_SCOPES {
                if *scope == SCOPE_SUPERUSER {
                    continue;
                }
                assert!(scope_granted(&eff, scope), "{role:?} should pass {scope}");
            }
            assert!(scope_granted(&eff, "users:write"));
            assert!(scope_granted(&eff, "loads:settle"));
        }
    }

    #[test]
    fn test_dispatcher_passes_operational_scopes() {
        let eff = effective_scopes(Role::Dispatcher, &[]);
        assert!(scope_granted(&eff, "loads:write"));
        assert!(scope_granted(&eff, "trips:delete"));
        assert!(scope_granted(&eff, "drivers:write"));
        assert!(scope_granted(&eff, "terminals:read"));
        assert!(scope_granted(&eff, "api_keys:delete"));
    }

    #[test]
    fn test_dispatcher_denied_elevated_and_users_scopes() {
        let eff = effective_scopes(Role::Dispatcher, &[]);
        assert!(!scope_granted(&eff, "users:read"));
        assert!(!scope_granted(&eff, "users:write"));
        assert!(!scope_granted(&eff, "loads:settle"));
        assert!(!scope_granted(&eff, "loads:invoice"));
        // No master-data deletes.
        assert!(!scope_granted(&eff, "loads:delete"));
        assert!(!scope_granted(&eff, "drivers:delete"));
        assert!(!scope_granted(&eff, "trucks:delete"));
        assert!(!scope_granted(&eff, "trailers:delete"));
        assert!(!scope_granted(&eff, "facilities:delete"));
        assert!(!scope_granted(&eff, "terminals:write"));
    }

    #[test]
    fn test_resource_wildcard_grants_action() {
        let eff = vec!["drivers:*".to_string()];
        assert!(scope_granted(&eff, "drivers:write"));
        assert!(scope_granted(&eff, "drivers:delete"));
        assert!(!scope_granted(&eff, "loads:write"));
    }

    #[test]
    fn test_global_wildcard_grants_all() {
        let eff = vec![SCOPE_SUPERUSER.to_string()];
        assert!(scope_granted(&eff, "users:delete"));
        assert!(scope_granted(&eff, "loads:settle"));
        assert!(scope_granted(&eff, "anything:goes"));
    }

    #[test]
    fn test_extra_scopes_elevate_exactly_one_scope() {
        let extra = vec!["loads:settle".to_string()];
        let eff = effective_scopes(Role::Dispatcher, &extra);
        // The granted one passes.
        assert!(scope_granted(&eff, "loads:settle"));
        // A sibling elevated scope does NOT leak.
        assert!(!scope_granted(&eff, "loads:invoice"));
        assert!(!scope_granted(&eff, "users:write"));
    }

    #[test]
    fn test_effective_scopes_dedupes() {
        let extra = vec!["loads:read".to_string(), "loads:read".to_string()];
        let eff = effective_scopes(Role::Dispatcher, &extra);
        assert_eq!(eff.iter().filter(|s| *s == "loads:read").count(), 1);
    }

    #[test]
    fn test_extra_is_noop_for_owner() {
        // Grants on top of `*` add nothing behaviorally — `*` already grants all.
        let eff = effective_scopes(Role::Owner, &["loads:settle".to_string()]);
        assert!(eff.contains(&SCOPE_SUPERUSER.to_string()));
        let owner_plain = effective_scopes(Role::Owner, &[]);
        for scope in ALL_SCOPES {
            assert_eq!(
                scope_granted(&eff, scope),
                scope_granted(&owner_plain, scope),
                "grant changed owner authority for {scope}",
            );
        }
    }
}
