//! Full-context, git-style patch text of each [`SourceFile`].

use similar::{Algorithm, ChangeTag, TextDiff};

use super::SourceFile;

fn quoted(name: &str) -> String {
    format!(
        "\"{}\"",
        name.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r")
            .replace('\t', "\\t")
    )
}

/// A one-hunk, full-context git-style patch for one file. Lines are written
/// verbatim (CRLF preserved), so `similar`'s own unified writer is not used.
pub fn full_context_patch(file: &SourceFile) -> String {
    let old_path = file.old_path.as_deref().unwrap_or(&file.path);
    let a = quoted(&format!("a/{old_path}"));
    let b = quoted(&format!("b/{}", file.path));
    let mut patch = format!("diff --git {a} {b}\n");
    if file.old_path.as_deref().is_some_and(|o| o != file.path) {
        patch.push_str(&format!(
            "rename from {}\nrename to {}\n",
            quoted(old_path),
            quoted(&file.path)
        ));
    }
    match (&file.old, &file.new) {
        (None, Some(_)) => patch.push_str("new file mode 100644\n"),
        (Some(_), None) => patch.push_str("deleted file mode 100644\n"),
        _ => {}
    }
    if file.binary {
        patch.push_str(&format!("Binary files {a} and {b} differ\n"));
        return patch;
    }
    let old = file.old.as_deref().unwrap_or("");
    let new = file.new.as_deref().unwrap_or("");
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let old_name = if file.old.is_some() {
        a
    } else {
        "/dev/null".into()
    };
    let new_name = if file.new.is_some() {
        b
    } else {
        "/dev/null".into()
    };
    patch.push_str(&format!("--- {old_name}\n+++ {new_name}\n"));
    if old_lines.is_empty() && new_lines.is_empty() {
        return patch;
    }
    patch.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        usize::from(!old_lines.is_empty()),
        old_lines.len(),
        usize::from(!new_lines.is_empty()),
        new_lines.len()
    ));
    // Patience keeps moved blocks of code readable; Myers is the fallback default.
    let diff = TextDiff::configure()
        .algorithm(Algorithm::Patience)
        .diff_slices(&old_lines, &new_lines);
    for op in diff.ops() {
        for change in diff.iter_changes(op) {
            patch.push(match change.tag() {
                ChangeTag::Equal => ' ',
                ChangeTag::Delete => '-',
                ChangeTag::Insert => '+',
            });
            patch.push_str(change.value());
            if !change.value().ends_with('\n') {
                patch.push_str("\n\\ No newline at end of file\n");
            }
        }
    }
    patch
}

/// All files as one patch.
pub fn patch_of(files: &[SourceFile]) -> String {
    files.iter().map(full_context_patch).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::file;

    #[test]
    fn patch_full_context_and_markers() {
        let p = full_context_patch(&file(Some("a\nb\nc\n"), Some("a\nB\nc\n")));
        assert!(p.starts_with("diff --git \"a/src/a.rs\" \"b/src/a.rs\"\n--- \"a/src/a.rs\"\n+++ \"b/src/a.rs\"\n@@ -1,3 +1,3 @@\n"));
        assert!(p.ends_with(" a\n-b\n+B\n c\n"), "{p}");
    }

    #[test]
    fn patch_added_deleted_no_newline_crlf() {
        let p = full_context_patch(&file(None, Some("x\r\ny")));
        assert!(p.contains("new file mode 100644\n--- /dev/null\n"));
        assert!(
            p.contains("@@ -0,0 +1,2 @@\n+x\r\n+y\n\\ No newline at end of file\n"),
            "{p}"
        );
        let p = full_context_patch(&file(Some("gone\n"), None));
        assert!(
            p.contains("deleted file mode 100644\n")
                && p.contains("+++ /dev/null\n@@ -1,1 +0,0 @@\n-gone\n")
        );
    }

    #[test]
    fn patch_rename_and_binary() {
        let mut f = file(Some("same\n"), Some("same\n"));
        f.old_path = Some("src/old.rs".into());
        let p = full_context_patch(&f);
        assert!(
            p.contains("rename from \"src/old.rs\"\nrename to \"src/a.rs\"\n"),
            "{p}"
        );
        let mut b = file(Some(""), Some(""));
        b.binary = true;
        assert!(full_context_patch(&b).contains("Binary files"));
    }

    /// Patch text of synthetic sources (no git): quoting, rename + edit,
    /// add/delete of empty files, binary rename, no newline at either end.
    #[test]
    fn patch_golden_synthetic() {
        let src =
            |path: &str, old_path: Option<&str>, old: Option<&str>, new: Option<&str>| SourceFile {
                path: path.into(),
                old_path: old_path.map(Into::into),
                old: old.map(Into::into),
                new: new.map(Into::into),
                binary: false,
            };
        let mut bin = src("img 2.png", Some("img.png"), Some(""), Some(""));
        bin.binary = true;
        let files = [
            src("q\"uote\\d.rs", None, Some("a\n"), Some("b\n")),
            src(
                "dir/new.rs",
                Some("dir/old.rs"),
                Some("x\ny\nz\n"),
                Some("x\nY\nz"),
            ),
            src("empty.txt", None, None, Some("")),
            src("gone-empty.txt", None, Some(""), None),
            bin,
            src("same.rs", Some("same.rs"), Some("k\n"), Some("k\n")),
            src("tail.txt", None, Some("p\nq"), Some("p\nq\n")),
        ];
        let got = patch_of(&files);
        let expected = r#"diff --git "a/q\"uote\\d.rs" "b/q\"uote\\d.rs"
--- "a/q\"uote\\d.rs"
+++ "b/q\"uote\\d.rs"
@@ -1,1 +1,1 @@
-a
+b
diff --git "a/dir/old.rs" "b/dir/new.rs"
rename from "dir/old.rs"
rename to "dir/new.rs"
--- "a/dir/old.rs"
+++ "b/dir/new.rs"
@@ -1,3 +1,3 @@
 x
-y
-z
+Y
+z
\ No newline at end of file
diff --git "a/empty.txt" "b/empty.txt"
new file mode 100644
--- /dev/null
+++ "b/empty.txt"
diff --git "a/gone-empty.txt" "b/gone-empty.txt"
deleted file mode 100644
--- "a/gone-empty.txt"
+++ /dev/null
diff --git "a/img.png" "b/img 2.png"
rename from "img.png"
rename to "img 2.png"
Binary files "a/img.png" and "b/img 2.png" differ
diff --git "a/same.rs" "b/same.rs"
--- "a/same.rs"
+++ "b/same.rs"
@@ -1,1 +1,1 @@
 k
diff --git "a/tail.txt" "b/tail.txt"
--- "a/tail.txt"
+++ "b/tail.txt"
@@ -1,2 +1,2 @@
 p
-q
\ No newline at end of file
+q
"#;
        assert_eq!(got, expected, "\n{got}");
    }
}
