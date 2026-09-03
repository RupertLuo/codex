use std::fs;
use std::sync::Arc;

use codex_config::ConfigLayerEntry;
use codex_config::ConfigLayerSource;
use codex_config::ConfigLayerStack;
use codex_config::ConfigRequirementsToml;
use codex_exec_server::LOCAL_FS;
use codex_protocol::protocol::SkillScope;
use codex_skills::SkillMetadata;
use codex_utils_absolute_path::AbsolutePathBuf;
use codex_utils_plugins::PluginIdentity;
use codex_utils_plugins::PluginSkillRoot;
use codex_utils_plugins::SkillDiscoveryMode;
use pretty_assertions::assert_eq;
use tempfile::TempDir;
use tokio::sync::Semaphore;

use super::resolve_skill_roots;
use super::roots_from_layer_stack;
use crate::loader::MAX_CONCURRENT_ROOT_SCANS;
use crate::loader::load_and_merge_host_skill_roots;

fn absolute(path: impl Into<std::path::PathBuf>) -> AbsolutePathBuf {
    AbsolutePathBuf::try_from(path.into()).expect("absolute path")
}

fn empty_config() -> toml::Value {
    toml::Value::Table(toml::map::Map::new())
}

fn stack(layers: Vec<ConfigLayerEntry>) -> ConfigLayerStack {
    ConfigLayerStack::new(
        layers,
        Default::default(),
        ConfigRequirementsToml::default(),
    )
    .expect("valid config stack")
}

fn user_layer(codex_home: &AbsolutePathBuf) -> ConfigLayerEntry {
    ConfigLayerEntry::new(
        ConfigLayerSource::User {
            file: codex_home.join("config.toml"),
            profile: None,
        },
        empty_config(),
    )
}

fn project_layer(dot_codex_folder: &AbsolutePathBuf) -> ConfigLayerEntry {
    ConfigLayerEntry::new(
        ConfigLayerSource::Project {
            dot_codex_folder: dot_codex_folder.clone(),
        },
        empty_config(),
    )
}

fn write_skill(root: &AbsolutePathBuf, directory: &str, name: &str) -> AbsolutePathBuf {
    let skill_dir = root.join(directory);
    fs::create_dir_all(&skill_dir).expect("create skill directory");
    let skill_path = skill_dir.join("SKILL.md");
    fs::write(
        &skill_path,
        format!("---\nname: {name}\ndescription: {name} description\n---\n"),
    )
    .expect("write skill");
    AbsolutePathBuf::from_absolute_path(
        dunce::canonicalize(skill_path).expect("canonical skill path"),
    )
    .expect("absolute skill path")
}

fn expected_skill(path: AbsolutePathBuf, name: &str, scope: SkillScope) -> SkillMetadata {
    SkillMetadata {
        name: name.to_string(),
        description: format!("{name} description"),
        short_description: None,
        interface: None,
        dependencies: None,
        policy: None,
        path_to_skills_md: path,
        scope,
        plugin_id: None,
        remote_plugin_id: None,
    }
}

#[test]
fn layer_roots_preserve_scope_precedence_and_disabled_projects() {
    let temp_dir = TempDir::new().expect("temp dir");
    let system_folder = absolute(temp_dir.path().join("etc/codex"));
    let home_folder = absolute(temp_dir.path().join("home"));
    let user_folder = home_folder.join("codex");
    let project_folder = absolute(temp_dir.path().join("repo/.codex"));
    let nested_project_folder = absolute(temp_dir.path().join("repo/nested/.codex"));
    let config_stack = stack(vec![
        ConfigLayerEntry::new(
            ConfigLayerSource::System {
                file: system_folder.join("config.toml"),
            },
            empty_config(),
        ),
        user_layer(&user_folder),
        ConfigLayerEntry::new_disabled(
            ConfigLayerSource::Project {
                dot_codex_folder: project_folder.clone(),
            },
            empty_config(),
            "untrusted project",
        ),
        project_layer(&nested_project_folder),
    ]);

    let roots = roots_from_layer_stack(&config_stack, Some(Arc::clone(&LOCAL_FS)))
        .into_iter()
        .map(|root| (root.scope, root.path))
        .collect::<Vec<_>>();

    assert_eq!(
        roots,
        vec![
            (SkillScope::Repo, nested_project_folder.join("skills")),
            (SkillScope::Repo, project_folder.join("skills")),
            (SkillScope::User, user_folder.join("skills")),
            (SkillScope::System, user_folder.join("skills/.system")),
            (SkillScope::Admin, system_folder.join("skills")),
        ]
    );
}

#[tokio::test]
async fn plugin_roots_preserve_plugin_resolution_metadata() {
    let temp_dir = TempDir::new().expect("temp dir");
    let cwd = absolute(temp_dir.path().join("workspace"));
    let plugin_root = absolute(temp_dir.path().join("plugins/example"));
    let skills_root = plugin_root.join("skills");
    let plugin_identity = PluginIdentity {
        plugin_id: "example@test".to_string(),
        remote_plugin_id: Some("plugins~Plugin_example".to_string()),
    };
    let plugin_namespace = "example".to_string();

    let roots = resolve_skill_roots(
        /*repository_file_system*/ None,
        &stack(Vec::new()),
        &cwd,
        vec![PluginSkillRoot {
            path: skills_root.clone(),
            plugin_identity: plugin_identity.clone(),
            plugin_namespace: plugin_namespace.clone(),
            plugin_root: plugin_root.clone(),
            discovery_mode: SkillDiscoveryMode::DirectChildren,
        }],
        Vec::new(),
    )
    .await;

    assert_eq!(roots.len(), 1);
    let root = &roots[0];
    assert_eq!(
        (
            root.path.clone(),
            root.scope,
            root.plugin_identity().cloned(),
            root.plugin_namespace().map(str::to_string),
            root.plugin_root().cloned(),
            root.discovery_mode(),
        ),
        (
            skills_root,
            SkillScope::User,
            Some(plugin_identity),
            Some(plugin_namespace),
            Some(plugin_root),
            SkillDiscoveryMode::DirectChildren,
        )
    );
    assert!(Arc::ptr_eq(&root.file_system, &LOCAL_FS));
}

#[tokio::test]
async fn unique_extra_root_loads_as_recursive_user_root() {
    let temp_dir = TempDir::new().expect("temp dir");
    let cwd = absolute(temp_dir.path().join("workspace"));
    let extra_root = absolute(temp_dir.path().join("runtime-skills"));
    let skill_path = write_skill(&extra_root, "runtime", "runtime-skill");

    let roots = resolve_skill_roots(
        /*repository_file_system*/ None,
        &stack(Vec::new()),
        &cwd,
        Vec::new(),
        vec![extra_root.clone()],
    )
    .await;

    assert_eq!(roots.len(), 1);
    let root = &roots[0];
    assert_eq!(
        (
            root.path.clone(),
            root.scope,
            root.plugin_identity().cloned(),
            root.plugin_namespace().map(str::to_string),
            root.plugin_root().cloned(),
            root.discovery_mode(),
        ),
        (
            extra_root,
            SkillScope::User,
            None,
            None,
            None,
            SkillDiscoveryMode::Recursive,
        )
    );
    assert!(Arc::ptr_eq(&root.file_system, &LOCAL_FS));

    let outcome = load_and_merge_host_skill_roots(
        roots,
        &Semaphore::new(MAX_CONCURRENT_ROOT_SCANS),
        /*restriction_product*/ None,
        /*plugin_skill_snapshots*/ None,
    )
    .await;

    assert!(outcome.errors.is_empty());
    assert_eq!(
        outcome.skills,
        vec![expected_skill(
            skill_path,
            "runtime-skill",
            SkillScope::User,
        )]
    );
}

#[tokio::test]
async fn resolved_project_layer_loads_skill_without_git_marker() {
    let temp_dir = TempDir::new().expect("temp dir");
    let workspace = absolute(temp_dir.path().join("workspace"));
    let dot_codex = workspace.join(".codex");
    let skill_root = dot_codex.join("skills");
    fs::create_dir_all(&workspace).expect("create workspace");
    let skill_path = write_skill(&skill_root, "local", "local-skill");
    let config_stack = stack(vec![project_layer(&dot_codex)]);

    let roots = resolve_skill_roots(
        Some(Arc::clone(&LOCAL_FS)),
        &config_stack,
        &workspace,
        Vec::new(),
        Vec::new(),
    )
    .await;
    let outcome = load_and_merge_host_skill_roots(
        roots,
        &Semaphore::new(MAX_CONCURRENT_ROOT_SCANS),
        /*restriction_product*/ None,
        /*plugin_skill_snapshots*/ None,
    )
    .await;

    assert!(outcome.errors.is_empty());
    assert_eq!(
        outcome.skills,
        vec![expected_skill(skill_path, "local-skill", SkillScope::Repo)]
    );
}

#[tokio::test]
async fn resolved_project_layer_loads_skill_when_cwd_is_file() {
    let temp_dir = TempDir::new().expect("temp dir");
    let repository = absolute(temp_dir.path().join("repo"));
    let dot_codex = repository.join(".codex");
    let skill_root = dot_codex.join("skills");
    fs::create_dir_all(&repository).expect("create repository");
    fs::write(repository.join(".git"), "gitdir: fake\n").expect("write git marker");
    let cwd = repository.join("some-file.txt");
    fs::write(&cwd, "contents").expect("write cwd file");
    let skill_path = write_skill(&skill_root, "repo", "repo-skill");
    let config_stack = stack(vec![project_layer(&dot_codex)]);

    let roots = resolve_skill_roots(
        Some(Arc::clone(&LOCAL_FS)),
        &config_stack,
        &cwd,
        Vec::new(),
        Vec::new(),
    )
    .await;
    let outcome = load_and_merge_host_skill_roots(
        roots,
        &Semaphore::new(MAX_CONCURRENT_ROOT_SCANS),
        /*restriction_product*/ None,
        /*plugin_skill_snapshots*/ None,
    )
    .await;

    assert!(outcome.errors.is_empty());
    assert_eq!(
        outcome.skills,
        vec![expected_skill(skill_path, "repo-skill", SkillScope::Repo)]
    );
}

#[tokio::test]
async fn resolved_roots_preserve_configured_sources_and_ignore_agents_dirs() {
    let temp_dir = TempDir::new().expect("temp dir");
    let home_folder = absolute(temp_dir.path().join("home"));
    let codex_home = home_folder.join("codex");
    let system_folder = absolute(temp_dir.path().join("etc/codex"));
    let repository = absolute(temp_dir.path().join("repo"));
    let cwd = repository.join("nested/inner");
    fs::create_dir_all(&cwd).expect("create cwd");
    fs::write(repository.join(".git"), "gitdir: fake\n").expect("write git marker");

    let project_dot_codex = repository.join(".codex");
    let nested_project_dot_codex = repository.join("nested/.codex");
    let user_skills = codex_home.join("skills");
    let root_project_skill = write_skill(
        &project_dot_codex.join("skills"),
        "root-duplicate",
        "duplicate-skill",
    );
    let nested_project_skill = write_skill(
        &nested_project_dot_codex.join("skills"),
        "nested-duplicate",
        "duplicate-skill",
    );
    let user_skill = write_skill(&user_skills, "user-duplicate", "duplicate-skill");
    write_skill(&home_folder.join(".agents/skills"), "home", "home-skill");
    let system_skill = write_skill(&codex_home.join("skills/.system"), "system", "system-skill");
    let admin_skill = write_skill(&system_folder.join("skills"), "admin", "admin-skill");
    write_skill(
        &repository.join(".agents/skills"),
        "repo-agent",
        "repo-agent-skill",
    );
    write_skill(
        &repository.join("nested/.agents/skills"),
        "nested-agent",
        "nested-agent-skill",
    );
    let config_stack = stack(vec![
        ConfigLayerEntry::new(
            ConfigLayerSource::System {
                file: system_folder.join("config.toml"),
            },
            empty_config(),
        ),
        user_layer(&codex_home),
        project_layer(&project_dot_codex),
        project_layer(&nested_project_dot_codex),
    ]);

    let roots = resolve_skill_roots(
        Some(Arc::clone(&LOCAL_FS)),
        &config_stack,
        &cwd,
        Vec::new(),
        vec![user_skills],
    )
    .await;
    assert_eq!(roots.len(), 5);
    let outcome = load_and_merge_host_skill_roots(
        roots,
        &Semaphore::new(MAX_CONCURRENT_ROOT_SCANS),
        /*restriction_product*/ None,
        /*plugin_skill_snapshots*/ None,
    )
    .await;
    assert!(outcome.errors.is_empty());
    assert_eq!(
        outcome.skills,
        vec![
            expected_skill(root_project_skill, "duplicate-skill", SkillScope::Repo),
            expected_skill(nested_project_skill, "duplicate-skill", SkillScope::Repo),
            expected_skill(user_skill, "duplicate-skill", SkillScope::User),
            expected_skill(system_skill, "system-skill", SkillScope::System),
            expected_skill(admin_skill, "admin-skill", SkillScope::Admin),
        ]
    );
}
