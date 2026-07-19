use super::{resolve, write_text_no_follow};
use std::{fs, os::unix::fs::symlink};

#[test]
fn rejects_dangling_symlink_leaf() {
    let base = std::env::temp_dir().join(format!("acp-hub-resolve-{}", uuid::Uuid::new_v4()));
    let root = base.join("root");
    let outside = base.join("outside.txt");
    fs::create_dir_all(&root).expect("create test root");
    symlink(&outside, root.join("link.txt")).expect("create dangling symlink");

    let result = resolve(&root.join("link.txt"), std::slice::from_ref(&root), &root);

    assert!(
        result.is_err(),
        "dangling symlink must not pass root confinement"
    );
    let _ = fs::remove_dir_all(&base);
}

#[test]
fn no_follow_open_rejects_leaf_swapped_after_resolve() {
    let base = std::env::temp_dir().join(format!("acp-hub-write-{}", uuid::Uuid::new_v4()));
    let root = base.join("root");
    let outside = base.join("outside.txt");
    fs::create_dir_all(&root).expect("create test root");
    let requested = root.join("new.txt");
    let resolved =
        resolve(&requested, std::slice::from_ref(&root), &root).expect("resolve new leaf");
    symlink(&outside, &requested).expect("swap leaf for dangling symlink");

    assert!(write_text_no_follow(&resolved, b"blocked").is_err());
    assert!(!outside.exists(), "outside target must not be created");
    let _ = fs::remove_dir_all(&base);
}
