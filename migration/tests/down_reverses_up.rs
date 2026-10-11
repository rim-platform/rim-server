use std::fs;

mod common;

/// Migrations whose `down` deliberately leaves the schema as it is.
const DOWN_NO_OP_ALLOWED: &[&str] = &[
    // Drops the incident module's tables with their rows; recreating them empty would bring
    // nothing back, so the way back is the `incidents-last` tag.
    "m20261010_184831_drop_incident_module",
];

/// Every migration defines its own `down` (the trait's default only returns an error), and that
/// `down` uses its schema manager, so rolling the migration back changes the schema. A `down`
/// that ignores the manager is a no-op and fails unless `DOWN_NO_OP_ALLOWED` names it. Reads the
/// source files only; no database needed.
#[test]
fn every_migration_has_a_down_that_undoes_its_up() {
    let mut missing = Vec::new();
    let mut no_op = Vec::new();
    let mut allowed_but_not_no_op = Vec::new();

    let sources = common::migration_sources();
    for (name, path) in &sources {
        let source = fs::read_to_string(path).expect("read migration source");
        let is_no_op = match down_fn(&strip_comments_and_strings(&source)) {
            None => {
                missing.push(name);
                continue;
            }
            Some(down) => down.is_no_op(),
        };
        let allowed = DOWN_NO_OP_ALLOWED.contains(&name.as_str());
        if is_no_op && !allowed {
            no_op.push(name);
        } else if !is_no_op && allowed {
            allowed_but_not_no_op.push(name);
        }
    }

    let unknown_allowed: Vec<&str> = DOWN_NO_OP_ALLOWED
        .iter()
        .copied()
        .filter(|n| !sources.contains_key(*n))
        .collect();

    assert!(
        missing.is_empty(),
        "migrations without their own `down` (the default refuses to roll back): {missing:?}"
    );
    assert!(
        no_op.is_empty(),
        "migrations whose `down` never touches the schema manager: {no_op:?}; write a `down` \
         that undoes `up`, or add the migration to DOWN_NO_OP_ALLOWED with a comment saying why"
    );
    assert!(
        allowed_but_not_no_op.is_empty(),
        "DOWN_NO_OP_ALLOWED names migrations whose `down` does undo `up`; remove them: \
         {allowed_but_not_no_op:?}"
    );
    assert!(
        unknown_allowed.is_empty(),
        "DOWN_NO_OP_ALLOWED names migrations that do not exist: {unknown_allowed:?}"
    );
}

#[test]
fn a_down_that_only_returns_ok_is_a_no_op() {
    let source = r#"
        impl MigrationTrait for Migration {
            async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> { manager.x().await }
            // The manager is mentioned only here: manager.drop_table()
            async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
                tracing::info!("manager {}", "{");
                Ok(())
            }
        }
    "#;
    let down = down_fn(&strip_comments_and_strings(source)).expect("down found");
    assert!(down.is_no_op());
}

#[test]
fn a_down_with_an_unused_manager_is_a_no_op() {
    let source = "async fn down(&self, _manager: &SchemaManager) -> R { warn!(); Ok(()) }";
    let down = down_fn(&strip_comments_and_strings(source)).expect("down found");
    assert!(down.is_no_op());
}

#[test]
fn a_down_that_uses_the_manager_is_not_a_no_op() {
    let source = r#"
        async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
            let conn = manager.get_connection();
            for sql in DOWN_SQL { conn.execute_unprepared(sql).await?; }
            Ok(())
        }
    "#;
    let down = down_fn(&strip_comments_and_strings(source)).expect("down found");
    assert!(!down.is_no_op());
}

#[test]
fn a_migration_without_down_has_none() {
    let source = "async fn up(&self, manager: &SchemaManager) -> R { manager.x() }";
    assert!(down_fn(&strip_comments_and_strings(source)).is_none());
}

/// A `down` function: the name of its schema-manager parameter and its body.
struct DownFn {
    manager_param: String,
    body: String,
}

impl DownFn {
    /// True when the body never uses the schema manager, so it cannot change the schema.
    fn is_no_op(&self) -> bool {
        self.manager_param.starts_with('_') || !contains_ident(&self.body, &self.manager_param)
    }
}

/// The `down` function in `source`, which must already be stripped of comments and strings.
fn down_fn(source: &str) -> Option<DownFn> {
    let start = source.find("fn down(")?;
    let after_params = &source[start + "fn down(".len()..];
    let params_end = after_params.find(')')?;
    let manager_param = after_params[..params_end]
        .split(',')
        .nth(1)?
        .split(':')
        .next()?
        .trim()
        .to_string();

    let open = start + source[start..].find('{')?;
    let mut depth = 0usize;
    for (i, c) in source[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(DownFn {
                        manager_param,
                        body: source[open + 1..open + i].to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether `ident` occurs in `text` as a whole identifier.
fn contains_ident(text: &str, ident: &str) -> bool {
    let is_ident_char = |c: char| c.is_ascii_alphanumeric() || c == '_';
    text.match_indices(ident).any(|(i, _)| {
        let before = text[..i].chars().next_back();
        let after = text[i + ident.len()..].chars().next();
        !before.is_some_and(is_ident_char) && !after.is_some_and(is_ident_char)
    })
}

/// `source` with comments removed and the contents of string literals (plain and raw) blanked,
/// so braces and words inside them do not count as code.
fn strip_comments_and_strings(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        if c == '/' && next == Some('/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && next == Some('*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i += 2;
        } else if c == 'r' && matches!(next, Some('"') | Some('#')) {
            let mut hashes = 0;
            let mut j = i + 1;
            while chars.get(j) == Some(&'#') {
                hashes += 1;
                j += 1;
            }
            if chars.get(j) != Some(&'"') {
                out.push(c);
                i += 1;
                continue;
            }
            j += 1;
            let closes_at = |k: usize| {
                chars.get(k) == Some(&'"') && (1..=hashes).all(|h| chars.get(k + h) == Some(&'#'))
            };
            while j < chars.len() && !closes_at(j) {
                j += 1;
            }
            out.push_str("\"\"");
            i = j + 1 + hashes;
        } else if c == '"' {
            i += 1;
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' {
                    i += 1;
                }
                i += 1;
            }
            out.push_str("\"\"");
            i += 1;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}
