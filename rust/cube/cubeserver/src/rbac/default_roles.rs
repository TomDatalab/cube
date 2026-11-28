//! `default_roles`: each organization gets its three default roles
//! (`owner`, `editor`, `viewer`), and every rule that still names a built-in
//! role moves to the organization's role of that key.

use super::authz::{Authz, Relationship};
use super::handlers::roles::{role_ref, wildcard_grants};
use super::handlers::rules::rule_ref;
use super::handlers::workspaces::workspace_ref;
use super::store::Store;

/// The marker recorded in `rbac_migrations` once a full pass completed.
pub const MARKER: &str = "default_roles";

/// Who the audit columns name for what the migration creates.
pub const CREATED_BY: &str = "system";

/// Seeds the default roles of every organization, unless the marker says it
/// was done, and moves the built-in rules onto them (on every start: an old
/// binary may have written one since). Returns how many rules it moved.
///
/// Every step is idempotent and ordered so a crash anywhere is repaired by
/// the next start: the new relationships are written before the old one is
/// deleted, and the row changes last.
pub async fn migrate_default_roles(store: &Store, authz: &dyn Authz) -> Result<usize, String> {
    let applied = store
        .migration_applied(MARKER)
        .await
        .map_err(|e| format!("Cannot read the migration marker {MARKER}: {e}"))?;
    let spicedb = |e| format!("Cannot write the default roles to SpiceDB (SPICEDB_ENDPOINT): {e}");
    let db = |e| format!("{MARKER}: cannot read or write the roles and rules: {e}");

    // Seeding runs once: a default role deleted later is not created again.
    let orgs = if applied { Vec::new() } else { store.organization_ids().await.map_err(db)? };
    for org in orgs {
        let roles = store.seed_default_roles(org, CREATED_BY).await.map_err(db)?;
        for role in &roles {
            let grants = wildcard_grants(role.id, &role.permissions);
            authz.touch(&grants).await.map_err(spicedb)?;
            for g in &grants {
                tracing::info!(
                    "{MARKER}: role:{}#{}@user:* written",
                    g.resource.id,
                    g.relation
                );
            }
        }
    }

    let mut moved = 0;
    for (rule, org) in store.rules_with_builtin_role().await.map_err(db)? {
        let Some(key) = rule.role.as_deref() else {
            continue;
        };
        let Some(role) = store
            .default_role(org, key)
            .await
            .map_err(db)?
        else {
            // The role was deleted: the rule keeps working through the
            // schema's `*_rule` relations, so it stays as it is.
            tracing::warn!(
                "{MARKER}: rule {} keeps its built-in {key} role: organization {org} has no {key} role",
                rule.id
            );
            continue;
        };
        let new = [
            Relationship {
                resource: workspace_ref(rule.workspace_id),
                relation: "rule",
                subject: rule_ref(rule.id),
            },
            Relationship {
                resource: rule_ref(rule.id),
                relation: "role",
                subject: role_ref(role.id),
            },
        ];
        let old = Relationship {
            resource: workspace_ref(rule.workspace_id),
            relation: match key {
                "owner" => "owner_rule",
                "editor" => "editor_rule",
                _ => "viewer_rule",
            },
            subject: rule_ref(rule.id),
        };
        authz.touch(&new).await.map_err(spicedb)?;
        tracing::info!(
            "{MARKER}: workspace:{}#rule@rule:{} and rule:{}#role@role:{} written ({key})",
            rule.workspace_id,
            rule.id,
            rule.id,
            role.id
        );
        authz
            .delete(std::slice::from_ref(&old))
            .await
            .map_err(spicedb)?;
        tracing::info!(
            "{MARKER}: workspace:{}#{}@rule:{} deleted",
            rule.workspace_id,
            old.relation,
            rule.id
        );
        store.set_rule_role_id(rule.id, role.id).await.map_err(db)?;
        moved += 1;
    }

    if !applied {
        store
            .record_migration(MARKER)
            .await
            .map_err(|e| format!("Cannot record the migration marker {MARKER}: {e}"))?;
    }
    if !applied || moved > 0 {
        tracing::info!("{MARKER}: {moved} rules moved to their organization's default roles");
    }
    Ok(moved)
}
