use std::collections::HashSet;
use std::sync::Arc;

use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_exec_server::ExecutorFileSystem;
use codex_exec_server::LOCAL_FS;
use codex_protocol::protocol::SkillScope;
use codex_skills::system_cache_root_dir;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_plugins::PluginSkillRoot;

use crate::loader::HostSkillRoot;

const SKILLS_DIR_NAME: &str = "skills";

/// Skill roots for a turn.
///
/// Neither the user's home directory nor the working directory is consulted: a skill reaches a
/// session only by being configured for it or shipped with the product.
pub(crate) async fn resolve_skill_roots(
    repository_file_system: Option<Arc<dyn ExecutorFileSystem>>,
    config_layer_stack: &ConfigLayerStack,
    _cwd: &AbsolutePathBuf,
    plugin_skill_roots: Vec<PluginSkillRoot>,
    extra_skill_roots: Vec<AbsolutePathBuf>,
) -> Vec<HostSkillRoot> {
    let mut roots = roots_from_layer_stack(config_layer_stack, repository_file_system);
    roots.extend(
        plugin_skill_roots
            .into_iter()
            .map(|root| HostSkillRoot::plugin(root, Arc::clone(&LOCAL_FS))),
    );
    roots.extend(
        extra_skill_roots
            .into_iter()
            .map(|path| local_root(path, SkillScope::User)),
    );
    dedupe_skill_roots_by_path(&mut roots);
    roots
}

fn roots_from_layer_stack(
    config_layer_stack: &ConfigLayerStack,
    repository_file_system: Option<Arc<dyn ExecutorFileSystem>>,
) -> Vec<HostSkillRoot> {
    let mut roots = Vec::new();

    for layer in config_layer_stack.all_layers_high_to_low() {
        let Some(config_folder) = layer.config_folder() else {
            continue;
        };

        match &layer.name {
            ConfigLayerSource::Project { .. } => {
                if let Some(repository_file_system) = &repository_file_system {
                    roots.push(HostSkillRoot::host(
                        config_folder.join(SKILLS_DIR_NAME),
                        SkillScope::Repo,
                        Arc::clone(repository_file_system),
                    ));
                }
            }
            ConfigLayerSource::User { .. } => {
                // Deprecated user skills location (`$CODEX_HOME/skills`), kept for backward
                // compatibility.
                roots.push(local_root(
                    config_folder.join(SKILLS_DIR_NAME),
                    SkillScope::User,
                ));

                // `$HOME/.agents/skills` is deliberately not a root. It is shared with whatever
                // else on the machine writes there, so honouring it lets skills the product never
                // shipped appear in its catalog and prompt.

                roots.push(local_root(
                    system_cache_root_dir(&config_folder),
                    SkillScope::System,
                ));
            }
            ConfigLayerSource::System { .. } => {
                roots.push(local_root(
                    config_folder.join(SKILLS_DIR_NAME),
                    SkillScope::Admin,
                ));
            }
            ConfigLayerSource::PackagedDefaults { .. }
            | ConfigLayerSource::Mdm { .. }
            | ConfigLayerSource::EnterpriseManaged { .. }
            | ConfigLayerSource::SessionFlags
            | ConfigLayerSource::LegacyManagedConfigTomlFromFile { .. }
            | ConfigLayerSource::LegacyManagedConfigTomlFromMdm => {}
        }
    }

    roots
}

fn local_root(path: AbsolutePathBuf, scope: SkillScope) -> HostSkillRoot {
    HostSkillRoot::host(path, scope, Arc::clone(&LOCAL_FS))
}

fn dedupe_skill_roots_by_path(roots: &mut Vec<HostSkillRoot>) {
    let mut seen = HashSet::new();
    roots.retain(|root| seen.insert(root.path.clone()));
}

#[cfg(test)]
#[path = "host_roots_tests.rs"]
mod tests;
