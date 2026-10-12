//! The root fields a GraphQL request's selected operation runs, found through named and inline
//! fragments at any depth.

use std::fmt;

use async_graphql::Positioned;
use async_graphql::parser::types::{
    DocumentOperations, ExecutableDocument, OperationDefinition, OperationType, Selection,
    SelectionSet,
};

/// The deepest fragment nesting followed: async-graphql's default recursion depth, past which it
/// refuses a document before running it.
pub const MAX_FRAGMENT_DEPTH: usize = 32;

/// The most root field occurrences counted. The schema's complexity limit refuses an operation
/// with far fewer.
pub const MAX_ROOT_FIELDS: usize = 1024;

/// Why a walk could not account for every root field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incomplete {
    /// No operation is selected: the operation name names none, or several operations and no
    /// name.
    NoOperation,
    /// A spread names a fragment the document does not define.
    UnknownFragment(String),
    /// A fragment spreads itself, directly or through others.
    FragmentCycle(String),
    /// Fragments nest deeper than [`MAX_FRAGMENT_DEPTH`].
    TooDeep,
    /// More than [`MAX_ROOT_FIELDS`] root field occurrences.
    TooLarge,
}

impl fmt::Display for Incomplete {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoOperation => f.write_str("no operation is selected"),
            Self::UnknownFragment(name) => write!(f, "unknown fragment {name}"),
            Self::FragmentCycle(name) => write!(f, "fragment {name} spreads itself"),
            Self::TooDeep => f.write_str("fragments nest too deep"),
            Self::TooLarge => f.write_str("too many root fields"),
        }
    }
}

/// Whether a walk accounts for every root field of the selected operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Classification {
    Complete,
    Incomplete(Incomplete),
}

/// The root fields of a request's selected operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationFields {
    /// The selected operation's type; `None` when no operation is selected.
    pub operation: Option<OperationType>,
    /// The document defines a mutation operation, selected or not.
    pub mutation_in_document: bool,
    /// Root field names, one per occurrence: an alias, a repeated field and each expansion of a
    /// fragment count separately. Fields under `@skip` or `@include` count as present. Only the
    /// fields found before the walk stopped when it is incomplete.
    pub fields: Vec<String>,
    pub classification: Classification,
}

impl OperationFields {
    /// Every root field is accounted for.
    pub fn is_complete(&self) -> bool {
        self.classification == Classification::Complete
    }

    /// The request may run a mutation: the selected operation is one, or none is selected and
    /// the document defines one.
    pub fn may_mutate(&self) -> bool {
        match self.operation {
            Some(ty) => ty == OperationType::Mutation,
            None => self.mutation_in_document,
        }
    }

    /// Root mutation fields the request runs, counted as [`OperationFields::fields`] are.
    pub fn mutation_count(&self) -> usize {
        if self.operation == Some(OperationType::Mutation) {
            self.fields.len()
        } else {
            0
        }
    }
}

/// Parses `query` and walks the operation `operation_name` selects; a parse error, which the
/// server refuses too, before anything else.
pub fn parse_root_fields(
    query: &str,
    operation_name: Option<&str>,
) -> Result<OperationFields, async_graphql::parser::Error> {
    let document = async_graphql::parser::parse_query(query)?;
    Ok(root_fields(&document, operation_name))
}

/// The root fields of the operation `operation_name` selects in `document`, chosen as
/// async-graphql chooses the operation it runs.
pub fn root_fields(document: &ExecutableDocument, operation_name: Option<&str>) -> OperationFields {
    let mutation_in_document = document
        .operations
        .iter()
        .any(|(_, op)| op.node.ty == OperationType::Mutation);
    let Some(operation) = select_operation(document, operation_name) else {
        return OperationFields {
            operation: None,
            mutation_in_document,
            fields: Vec::new(),
            classification: Classification::Incomplete(Incomplete::NoOperation),
        };
    };
    let mut walk = Walk {
        document,
        path: Vec::new(),
        fields: Vec::new(),
    };
    let classification = match walk.selection_set(&operation.node.selection_set.node, 0) {
        Ok(()) => Classification::Complete,
        Err(reason) => Classification::Incomplete(reason),
    };
    OperationFields {
        operation: Some(operation.node.ty),
        mutation_in_document,
        fields: walk.fields,
        classification,
    }
}

/// The type of the operation async-graphql runs for `operation_name`, or `None` where it refuses
/// the request.
pub fn selected_operation_type(
    document: &ExecutableDocument,
    operation_name: Option<&str>,
) -> Option<OperationType> {
    select_operation(document, operation_name).map(|operation| operation.node.ty)
}

/// The operation async-graphql runs for `operation_name`, or `None` where it refuses the request.
fn select_operation<'a>(
    document: &'a ExecutableDocument,
    operation_name: Option<&str>,
) -> Option<&'a Positioned<OperationDefinition>> {
    match (&document.operations, operation_name) {
        (DocumentOperations::Single(operation), None) => Some(operation),
        (DocumentOperations::Single(_), Some(_)) => None,
        (DocumentOperations::Multiple(operations), Some(name)) => operations.get(name),
        (DocumentOperations::Multiple(operations), None) if operations.len() == 1 => {
            operations.values().next()
        }
        (DocumentOperations::Multiple(_), None) => None,
    }
}

/// A depth-first walk of an operation's root selection set.
struct Walk<'a> {
    document: &'a ExecutableDocument,
    /// The fragments spread on the way to the current selection set, for cycles.
    path: Vec<&'a str>,
    fields: Vec<String>,
}

impl<'a> Walk<'a> {
    fn selection_set(&mut self, set: &'a SelectionSet, depth: usize) -> Result<(), Incomplete> {
        for selection in &set.items {
            match &selection.node {
                Selection::Field(field) => {
                    if self.fields.len() >= MAX_ROOT_FIELDS {
                        return Err(Incomplete::TooLarge);
                    }
                    self.fields.push(field.node.name.node.to_string());
                }
                Selection::InlineFragment(fragment) => {
                    self.descend(depth)?;
                    self.selection_set(&fragment.node.selection_set.node, depth + 1)?;
                }
                Selection::FragmentSpread(spread) => {
                    self.descend(depth)?;
                    let name = spread.node.fragment_name.node.as_str();
                    let fragment = self
                        .document
                        .fragments
                        .get(name)
                        .ok_or_else(|| Incomplete::UnknownFragment(name.to_string()))?;
                    if self.path.contains(&name) {
                        return Err(Incomplete::FragmentCycle(name.to_string()));
                    }
                    self.path.push(name);
                    self.selection_set(&fragment.node.selection_set.node, depth + 1)?;
                    self.path.pop();
                }
            }
        }
        Ok(())
    }

    fn descend(&self, depth: usize) -> Result<(), Incomplete> {
        if depth >= MAX_FRAGMENT_DEPTH {
            return Err(Incomplete::TooDeep);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use async_graphql::{EmptySubscription, Object, Schema};

    use super::*;

    /// The review's document: an exempt field in sight, a protected one two fragments down.
    const NESTED_PROTECTED: &str = r#"mutation {
  logout @skip(if: true)
  ...Outer
}
fragment Outer on Mutation { ...Inner }
fragment Inner on Mutation {
  updateMyProfile(input: { statusText: "changed without CSRF" }) { id }
}"#;

    fn walk(query: &str, operation_name: Option<&str>) -> OperationFields {
        parse_root_fields(query, operation_name).expect("parses")
    }

    fn names(fields: &OperationFields) -> Vec<&str> {
        fields.fields.iter().map(String::as_str).collect()
    }

    /// `depth` inline fragments around one field.
    fn nested_inline(depth: usize) -> String {
        format!(
            "mutation {{ {}logout{} }}",
            "... on Mutation { ".repeat(depth),
            " }".repeat(depth)
        )
    }

    #[test]
    fn nested_named_fragments_are_resolved() {
        let fields = walk(NESTED_PROTECTED, None);
        assert_eq!(names(&fields), ["logout", "updateMyProfile"]);
        assert!(fields.is_complete());
        assert_eq!(fields.operation, Some(OperationType::Mutation));
        assert_eq!(fields.mutation_count(), 2);
    }

    #[test]
    fn nested_inline_fragments_are_resolved() {
        let fields = walk(
            "mutation { ... on Mutation { ... { ... on Mutation { a: logout } } } login }",
            None,
        );
        assert_eq!(names(&fields), ["logout", "login"]);
        assert!(fields.is_complete());
    }

    #[test]
    fn aliases_and_repeated_expansions_count_each_occurrence() {
        let fields = walk(
            "mutation { a: logout b: logout ...F ...F } fragment F on Mutation { c: logout ...G } fragment G on Mutation { logout }",
            None,
        );
        assert_eq!(fields.fields.len(), 6);
        assert!(fields.is_complete());
    }

    #[test]
    fn a_fragment_cycle_is_incomplete() {
        let fields = walk(
            "mutation { ...A } fragment A on Mutation { logout ...B } fragment B on Mutation { ...A }",
            None,
        );
        assert_eq!(
            fields.classification,
            Classification::Incomplete(Incomplete::FragmentCycle("A".to_string()))
        );
        assert!(fields.may_mutate());
    }

    #[test]
    fn an_unknown_fragment_is_incomplete() {
        let fields = walk("mutation { logout ...Missing }", None);
        assert_eq!(
            fields.classification,
            Classification::Incomplete(Incomplete::UnknownFragment("Missing".to_string()))
        );
    }

    #[test]
    fn nesting_up_to_the_bound_is_followed_and_past_it_is_incomplete() {
        assert!(walk(&nested_inline(MAX_FRAGMENT_DEPTH), None).is_complete());
        let fields = walk(&nested_inline(MAX_FRAGMENT_DEPTH + 1), None);
        assert_eq!(
            fields.classification,
            Classification::Incomplete(Incomplete::TooDeep)
        );
    }

    #[test]
    fn exponential_expansion_stops_at_the_field_bound() {
        let mut query = String::from("mutation { ...F0 }");
        for level in 0..20 {
            query.push_str(&format!(
                " fragment F{level} on Mutation {{ ...F{next} ...F{next} }}",
                next = level + 1
            ));
        }
        query.push_str(" fragment F20 on Mutation { logout }");
        let fields = walk(&query, None);
        assert_eq!(
            fields.classification,
            Classification::Incomplete(Incomplete::TooLarge)
        );
        assert_eq!(fields.fields.len(), MAX_ROOT_FIELDS);
    }

    #[test]
    fn the_operation_name_selects_the_operation() {
        let query = "query Q { me } mutation M { updateMyProfile } mutation N { logout }";
        let m = walk(query, Some("M"));
        assert_eq!(names(&m), ["updateMyProfile"]);
        assert_eq!(m.operation, Some(OperationType::Mutation));
        let q = walk(query, Some("Q"));
        assert_eq!(names(&q), ["me"]);
        assert!(!q.may_mutate());
        assert_eq!(q.mutation_count(), 0);

        for name in [None, Some("Missing")] {
            let none = walk(query, name);
            assert_eq!(
                none.classification,
                Classification::Incomplete(Incomplete::NoOperation)
            );
            assert!(none.may_mutate(), "{name:?}");
            assert_eq!(none.mutation_count(), 0);
        }
        // A name for an anonymous operation selects nothing, as async-graphql refuses it.
        assert_eq!(walk("mutation { logout }", Some("M")).operation, None);
        assert_eq!(
            walk("mutation M { logout }", None).operation,
            Some(OperationType::Mutation)
        );
    }

    #[test]
    fn a_parse_error_is_an_error() {
        assert!(parse_root_fields("mutation {", None).is_err());
    }

    struct Query;

    #[Object]
    impl Query {
        async fn ok(&self) -> bool {
            true
        }
    }

    struct Mutation;

    #[Object]
    impl Mutation {
        async fn logout(&self) -> bool {
            true
        }
    }

    #[tokio::test]
    async fn async_graphql_refuses_what_lies_past_the_depth_bound() {
        let schema = Schema::new(Query, Mutation, EmptySubscription);
        let within = schema.execute(nested_inline(MAX_FRAGMENT_DEPTH)).await;
        assert!(within.errors.is_empty(), "{:?}", within.errors);
        let past = schema.execute(nested_inline(MAX_FRAGMENT_DEPTH + 1)).await;
        assert!(!past.errors.is_empty());
        assert_eq!(past.data, async_graphql::Value::Null);
    }
}
