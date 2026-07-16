//! wit-parser 0.247 layout constraint, discovered during S1: a WIT text of
//! ONLY nested `package … { }` blocks is rejected ("no `package` header was
//! found…") — the group's MAIN package must have a top-level header. The
//! spike's texts therefore use: root header + world at top level, deps nested.

use wit_parser::{Resolve, UnresolvedPackageGroup};

#[test]
fn nested_only_is_rejected_but_header_plus_nested_parses() {
    let nested_only = r#"package demo:host@0.1.0 {
    interface env {
        sleep: async func(ms: u64);
    }
}
"#;
    let group = UnresolvedPackageGroup::parse("probe.wit", nested_only).expect("parse");
    let mut resolve = Resolve::default();
    assert!(
        resolve.push_group(group).is_err(),
        "0.247 requires a top-level package header; if this starts passing, \
         the wit.rs texts can be simplified"
    );

    let with_header = r#"package demo:probe@0.1.0;

package demo:host@0.1.0 {
    interface env {
        sleep: async func(ms: u64);
    }
}

world probe {
    import demo:host/env@0.1.0;
}
"#;
    let group = UnresolvedPackageGroup::parse("probe.wit", with_header).expect("parse");
    let mut resolve = Resolve::default();
    let pkg = resolve.push_group(group).expect("push");
    resolve
        .select_world(&[pkg], Some("probe"))
        .expect("world resolves");
}
