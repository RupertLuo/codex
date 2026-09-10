use std::fs;
use std::path::Path;

use codex_exec_server::LOCAL_FS;
use codex_exec_server::WalkOptions;
use codex_utils_path_uri::PathUri;
use codex_utils_plugins::SkillDiscoveryMode;
use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::super::io_test_support::ManifestMetadataBehavior;
use super::super::io_test_support::RecordingFileSystem;
use super::DirectorySymlinkPolicy;
use super::HiddenDirectoryPolicy;
use super::SkillDiscovery;
use super::SkillDiscoveryOptions;
use super::discover_skills;

fn write_skill(root: &Path, directory: &str) {
    let path = root.join(directory).join("SKILL.md");
    fs::create_dir_all(path.parent().expect("skill parent")).expect("create skill directory");
    fs::write(path, "---\nname: demo\ndescription: Demo skill\n---\n")
        .expect("write skill frontmatter");
}

fn canonical_uri(path: &Path) -> PathUri {
    PathUri::from_host_native_path(fs::canonicalize(path).expect("canonical path"))
        .expect("absolute path URI")
}

async fn discover(root: &Path) -> SkillDiscovery {
    discover_skills(
        LOCAL_FS.as_ref(),
        &canonical_uri(root),
        SkillDiscoveryOptions {
            directory_symlinks: DirectorySymlinkPolicy::Follow,
            hidden_directories: HiddenDirectoryPolicy::Skip,
            mode: SkillDiscoveryMode::Recursive,
        },
    )
    .await
}

#[cfg(unix)]
#[tokio::test]
async fn discovers_hidden_directory_through_visible_symlink() {
    let root = TempDir::new().expect("root temp dir");
    write_skill(root.path(), ".hidden/search");
    std::os::unix::fs::symlink(root.path().join(".hidden"), root.path().join("visible"))
        .expect("create visible skill directory symlink");

    let discovery = discover(root.path()).await;

    assert_eq!(discovery.warnings, Vec::<String>::new());
    assert_eq!(
        discovery
            .skills
            .into_iter()
            .map(|skill| skill.path)
            .collect::<Vec<_>>(),
        vec![
            canonical_uri(root.path())
                .join("visible/search/SKILL.md")
                .expect("visible skill URI"),
        ]
    );
}

#[cfg(unix)]
#[tokio::test]
async fn ignores_symlinked_skill_files_and_directory_cycles() {
    let root = TempDir::new().expect("root temp dir");
    let external = TempDir::new().expect("external temp dir");
    write_skill(external.path(), "external");
    write_skill(root.path(), "cycle/demo");
    fs::create_dir_all(root.path().join("linked-file")).expect("create linked skill directory");
    std::os::unix::fs::symlink(
        external.path().join("external/SKILL.md"),
        root.path().join("linked-file/SKILL.md"),
    )
    .expect("create skill file symlink");
    std::os::unix::fs::symlink(root.path().join("cycle"), root.path().join("cycle/loop"))
        .expect("create directory symlink cycle");

    let discovery = tokio::time::timeout(
        std::time::Duration::from_secs(/*secs*/ 5),
        discover(root.path()),
    )
    .await
    .expect("directory symlink cycles must not prevent discovery from completing");

    assert_eq!(
        discovery
            .skills
            .into_iter()
            .map(|skill| skill.path)
            .collect::<Vec<_>>(),
        vec![
            canonical_uri(root.path())
                .join("cycle/demo/SKILL.md")
                .expect("valid skill URI"),
        ]
    );
}

#[tokio::test]
async fn respects_maximum_skill_scan_depth() {
    let root = TempDir::new().expect("root temp dir");
    let within_depth = "d0/d1/d2/d3/d4/d5";
    let beyond_depth = "d0/d1/d2/d3/d4/d5/d6";
    write_skill(root.path(), within_depth);
    write_skill(root.path(), beyond_depth);

    let discovery = discover(root.path()).await;

    assert_eq!(
        discovery
            .skills
            .into_iter()
            .map(|skill| skill.path)
            .collect::<Vec<_>>(),
        vec![
            canonical_uri(root.path())
                .join(&format!("{within_depth}/SKILL.md"))
                .expect("reachable skill URI"),
        ]
    );
}

#[tokio::test]
async fn discovers_nested_skills_when_resource_inventory_exceeds_response_budget() {
    let root = TempDir::new().expect("root temp dir");
    write_skill(root.path(), "pack");
    write_skill(root.path(), "pack/assets/z-nested");
    let assets = root.path().join("pack/assets");
    // Even without the root path, these filenames and per-entry overhead exceed 4 MiB,
    // while the complete inventory remains below the unchanged 20,000-entry limit.
    for index in 0..16_000 {
        let name = format!("x{index:05}_{}.bin", "r".repeat(192));
        fs::write(assets.join(name), "").expect("write resource fixture");
    }
    let metadata_dir = root.path().join("pack/agents");
    fs::create_dir_all(&metadata_dir).expect("create metadata directory");
    fs::write(metadata_dir.join("openai.yaml"), "").expect("write metadata fixture");
    let root_uri = canonical_uri(root.path());
    let unfiltered = LOCAL_FS
        .walk(
            &root_uri,
            WalkOptions {
                max_depth: super::MAX_SCAN_DEPTH,
                max_directories: super::MAX_SKILLS_DIRS_PER_ROOT,
                max_entries: super::MAX_SKILLS_ENTRIES_PER_ROOT,
                follow_directory_symlinks: true,
                prune_hidden_directories: true,
                file_names: None,
            },
            /*sandbox*/ None,
        )
        .await
        .expect("walk unfiltered fixture");
    assert!(unfiltered.truncated);
    let nested_skill = root_uri
        .join("pack/assets/z-nested/SKILL.md")
        .expect("nested skill URI");
    assert!(
        !unfiltered
            .entries
            .iter()
            .any(|entry| entry.path == nested_skill)
    );

    let mut legacy_file_system =
        RecordingFileSystem::new(LOCAL_FS.as_ref(), ManifestMetadataBehavior::Immediate);
    legacy_file_system.ignore_walk_file_filter = true;
    let legacy_discovery = discover_skills(
        &legacy_file_system,
        &root_uri,
        SkillDiscoveryOptions {
            directory_symlinks: DirectorySymlinkPolicy::Follow,
            hidden_directories: HiddenDirectoryPolicy::Skip,
            mode: SkillDiscoveryMode::Recursive,
        },
    )
    .await;
    assert_eq!(legacy_discovery.skills.len(), 1);
    assert_eq!(
        legacy_discovery.warnings,
        vec![format!(
            "skills scan reached its traversal limit (root: {root_uri})"
        )],
    );
    assert!(matches!(
        legacy_discovery.skills[0].metadata,
        super::SkillMetadataDiscovery::Present(_)
    ));

    let discovery = discover(root.path()).await;

    assert_eq!(discovery.warnings, Vec::<String>::new());
    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| skill.path.clone())
            .collect::<Vec<_>>(),
        vec![
            root_uri.join("pack/SKILL.md").expect("parent skill URI"),
            nested_skill
        ],
    );
    assert!(matches!(
        &discovery.skills[0].metadata,
        super::SkillMetadataDiscovery::Present(path)
            if path == &root_uri.join("pack/agents/openai.yaml").expect("metadata URI")
    ));
}

#[tokio::test]
async fn discovers_skills_from_complete_legacy_unfiltered_inventory() {
    let root = TempDir::new().expect("root temp dir");
    write_skill(root.path(), "demo");
    fs::write(root.path().join("demo/resource.txt"), "resource").expect("write resource");
    let mut file_system =
        RecordingFileSystem::new(LOCAL_FS.as_ref(), ManifestMetadataBehavior::Immediate);
    file_system.ignore_walk_file_filter = true;
    let root_uri = canonical_uri(root.path());
    let discovery = discover_skills(
        &file_system,
        &root_uri,
        SkillDiscoveryOptions {
            directory_symlinks: DirectorySymlinkPolicy::Follow,
            hidden_directories: HiddenDirectoryPolicy::Skip,
            mode: SkillDiscoveryMode::Recursive,
        },
    )
    .await;
    assert_eq!(discovery.warnings, Vec::<String>::new());
    assert_eq!(
        discovery
            .skills
            .iter()
            .map(|skill| skill.path.clone())
            .collect::<Vec<_>>(),
        vec![root_uri.join("demo/SKILL.md").expect("skill URI")],
    );
    assert!(matches!(
        discovery.skills[0].metadata,
        super::SkillMetadataDiscovery::Absent
    ));
}
