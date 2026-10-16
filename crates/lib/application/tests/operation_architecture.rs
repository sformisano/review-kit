use std::{
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fs,
    path::{Path, PathBuf},
};

use syn::{Attribute, Item, Type, UseTree, visit::Visit};

const ROOTS: &[&str] = &[
    "crates/lib/application/src",
    "crates/bin/cli/src",
    "crates/lib/infrastructure/filesystem-adapter/src",
    "crates/lib/infrastructure/gitlab-adapter/src",
    "crates/lib/infrastructure/json-adapter/src",
    "crates/lib/infrastructure/json2md-adapter/src",
];

// These files contain value construction, codecs or disclosure projections.
// New files require declarations by default; exemptions need source review.
const PURE_FILES: &[&str] = &[
    "crates/lib/application/src/conversation.rs",
    "crates/lib/application/src/publication.rs",
    "crates/lib/application/src/review_run.rs",
    "crates/lib/application/src/observation.rs",
    "crates/lib/application/src/observation/errors.rs",
    "crates/bin/cli/src/args.rs",
    "crates/bin/cli/src/effects.rs",
    "crates/bin/cli/src/error.rs",
    "crates/bin/cli/src/logging.rs",
    "crates/bin/cli/src/commands/status.rs",
    "crates/lib/infrastructure/filesystem-adapter/src/observation.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/body.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/config.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/discovery/mapping.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/discovery/records.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/error.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/observation.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/source/diff.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/source/mapping.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/source/position.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/wire.rs",
    "crates/lib/infrastructure/gitlab-adapter/src/wire/rounds.rs",
    "crates/lib/infrastructure/json-adapter/src/checkpoints/acceptance.rs",
    "crates/lib/infrastructure/json-adapter/src/checkpoints/diagnostics.rs",
    "crates/lib/infrastructure/json-adapter/src/checkpoints/run.rs",
    "crates/lib/infrastructure/json-adapter/src/checkpoints/wire.rs",
    "crates/lib/infrastructure/json-adapter/src/checkpoints/workspace.rs",
    "crates/lib/infrastructure/json-adapter/src/diagnostics.rs",
    "crates/lib/infrastructure/json-adapter/src/domain.rs",
    "crates/lib/infrastructure/json-adapter/src/observation.rs",
    "crates/lib/infrastructure/json-adapter/src/reporting.rs",
    "crates/lib/infrastructure/json2md-adapter/src/observation.rs",
    "crates/lib/infrastructure/json2md-adapter/src/projection.rs",
];

const RAW_PORTS: &[&str] = &[
    "ReviewReader",
    "PublicationHost",
    "PublicationSession",
    "CommittedSource",
    "ReviewRenderer",
    "ReviewChanges",
    "AssessmentDraftWriter",
    "ExecutionEvidence",
];

#[derive(Debug, PartialEq, Eq)]
enum Violation {
    UndeclaredFunction(String),
    UndeclaredImplementation,
    MissingPolicy(String),
    RawPortField(String),
    DirectLifecycle(String),
    UnmanagedSession,
}

fn marked(attributes: &[Attribute], name: &str) -> bool {
    attributes.iter().any(|attribute| {
        attribute
            .path()
            .segments
            .last()
            .is_some_and(|s| s.ident == name)
    })
}

fn test_only(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute
                .meta
                .require_list()
                .is_ok_and(|list| list.tokens.to_string() == "test")
    })
}

fn item_attributes(item: &Item) -> &[Attribute] {
    match item {
        Item::Fn(item) => &item.attrs,
        Item::Impl(item) => &item.attrs,
        Item::Mod(item) => &item.attrs,
        Item::Struct(item) => &item.attrs,
        Item::Enum(item) => &item.attrs,
        Item::Trait(item) => &item.attrs,
        Item::Use(item) => &item.attrs,
        Item::Type(item) => &item.attrs,
        Item::Const(item) => &item.attrs,
        Item::Static(item) => &item.attrs,
        Item::Macro(item) => &item.attrs,
        _ => &[],
    }
}

fn module_directory(path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    let parent = path.parent().ok_or("source has no parent")?;
    match path.file_name().and_then(|name| name.to_str()) {
        Some("mod.rs" | "lib.rs" | "main.rs") => Ok(parent.to_owned()),
        _ => Ok(parent.join(path.file_stem().ok_or("source has no stem")?)),
    }
}

fn test_modules(
    items: &[Item],
    explicit_directory: &Path,
    default_directory: &Path,
    excluded: &mut BTreeSet<PathBuf>,
) -> Result<(), Box<dyn Error>> {
    for item in items {
        let Item::Mod(module) = item else { continue };
        if let Some((_, items)) = &module.content {
            if !test_only(&module.attrs) {
                let nested = default_directory.join(module.ident.to_string());
                test_modules(items, &nested, &nested, excluded)?;
            }
        } else if test_only(&module.attrs) {
            let explicit = module.attrs.iter().find_map(|attribute| {
                if attribute.path().is_ident("path")
                    && let syn::Meta::NameValue(value) = &attribute.meta
                    && let syn::Expr::Lit(value) = &value.value
                    && let syn::Lit::Str(value) = &value.lit
                {
                    Some(value.value())
                } else {
                    None
                }
            });
            let path = match explicit {
                Some(path) => explicit_directory.join(path),
                None => {
                    let direct = default_directory.join(format!("{}.rs", module.ident));
                    if direct.exists() {
                        direct
                    } else {
                        default_directory
                            .join(module.ident.to_string())
                            .join("mod.rs")
                    }
                }
            };
            excluded.insert(path.canonicalize()?);
        }
    }
    Ok(())
}

fn walk_sources(
    directory: &Path,
    files: &mut BTreeMap<PathBuf, syn::File>,
) -> Result<(), Box<dyn Error>> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            walk_sources(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            files.insert(
                path.canonicalize()?,
                syn::parse_file(&fs::read_to_string(&path)?)?,
            );
        }
    }
    Ok(())
}

fn production_sources() -> Result<(PathBuf, BTreeMap<PathBuf, syn::File>), Box<dyn Error>> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()?;
    let mut files = BTreeMap::new();
    for directory in ROOTS {
        walk_sources(&root.join(directory), &mut files)?;
    }
    let mut excluded = BTreeSet::new();
    for (path, file) in &files {
        test_modules(
            &file.items,
            path.parent().ok_or("source has no parent")?,
            &module_directory(path)?,
            &mut excluded,
        )?;
    }
    files.retain(|path, _| !excluded.contains(path));
    Ok((root, files))
}

struct Surface<'a> {
    path: &'a str,
    pure: bool,
    managed_module: bool,
    managed_impl: bool,
    managed_trait: bool,
    violations: Vec<Violation>,
}

impl Surface<'_> {
    fn function(&mut self, name: &syn::Ident, attributes: &[Attribute], managed: bool) {
        if !self.pure {
            if !managed {
                self.violations
                    .push(Violation::UndeclaredFunction(name.to_string()));
            } else if !marked(attributes, "operation") && !marked(attributes, "helper") {
                self.violations
                    .push(Violation::MissingPolicy(name.to_string()));
            }
        }
    }
}

impl<'ast> Visit<'ast> for Surface<'_> {
    fn visit_item(&mut self, item: &'ast Item) {
        if !test_only(item_attributes(item)) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_item_mod(&mut self, module: &'ast syn::ItemMod) {
        let previous = self.managed_module;
        self.managed_module |= marked(&module.attrs, "operations");
        syn::visit::visit_item_mod(self, module);
        self.managed_module = previous;
    }

    fn visit_item_fn(&mut self, function: &'ast syn::ItemFn) {
        self.function(
            &function.sig.ident,
            &function.attrs,
            self.managed_module || marked(&function.attrs, "operations"),
        );
        let previous = self.managed_module;
        self.managed_module = false;
        syn::visit::visit_item_fn(self, function);
        self.managed_module = previous;
    }

    fn visit_item_impl(&mut self, implementation: &'ast syn::ItemImpl) {
        let previous = (self.managed_impl, self.pure);
        // These root impls contain an error constructor and a clock getter.
        if self.path == "crates/lib/infrastructure/filesystem-adapter/src/lib.rs"
            && implementation.items.len() == 1
            && matches!((implementation.self_ty.as_ref(), implementation.items.first()),
                (Type::Path(path), Some(syn::ImplItem::Fn(method)))
                if path.path.is_ident("FileError") && method.sig.ident == "io"
                    || path.path.is_ident("SystemClock") && method.sig.ident == "now")
        {
            self.pure = true;
        }
        self.managed_impl = self.managed_module || marked(&implementation.attrs, "operations");
        if !self.pure && !self.managed_impl {
            self.violations.push(Violation::UndeclaredImplementation);
        }
        syn::visit::visit_item_impl(self, implementation);
        (self.managed_impl, self.pure) = previous;
    }

    fn visit_impl_item_fn(&mut self, function: &'ast syn::ImplItemFn) {
        self.function(&function.sig.ident, &function.attrs, self.managed_impl);
        syn::visit::visit_impl_item_fn(self, function);
    }

    fn visit_item_trait(&mut self, declaration: &'ast syn::ItemTrait) {
        let previous = self.managed_trait;
        self.managed_trait = marked(&declaration.attrs, "observed_port");
        syn::visit::visit_item_trait(self, declaration);
        self.managed_trait = previous;
    }

    fn visit_trait_item_fn(&mut self, function: &'ast syn::TraitItemFn) {
        if function.default.is_some() {
            self.function(&function.sig.ident, &function.attrs, self.managed_trait);
        }
        syn::visit::visit_trait_item_fn(self, function);
    }

    fn visit_expr_path(&mut self, expression: &'ast syn::ExprPath) {
        if expression
            .path
            .segments
            .iter()
            .any(|segment| segment.ident == "__private")
            || expression
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "observe")
        {
            self.violations
                .push(Violation::DirectLifecycle("path".into()));
        }
        syn::visit::visit_expr_path(self, expression);
    }

    fn visit_item_use(&mut self, item: &'ast syn::ItemUse) {
        fn forbidden(tree: &UseTree, observation: bool) -> bool {
            match tree {
                UseTree::Path(path) => {
                    path.ident == "__private"
                        || forbidden(
                            &path.tree,
                            observation || path.ident == "review_kit_observation",
                        )
                }
                UseTree::Name(name) => name.ident == "observe" || name.ident == "__private",
                UseTree::Rename(name) => name.ident == "observe" || name.ident == "__private",
                UseTree::Group(group) => {
                    group.items.iter().any(|tree| forbidden(tree, observation))
                }
                UseTree::Glob(_) => observation,
            }
        }
        if forbidden(&item.tree, false) {
            self.violations
                .push(Violation::DirectLifecycle("import".into()));
        }
    }
}

fn inspect(path: &str, file: &syn::File) -> Vec<Violation> {
    let mut visitor = Surface {
        path,
        pure: PURE_FILES.contains(&path),
        managed_module: false,
        managed_impl: false,
        managed_trait: false,
        violations: Vec::new(),
    };
    visitor.visit_file(file);
    visitor.violations
}

struct RawType<'a> {
    names: &'a BTreeSet<String>,
    found: bool,
}
impl<'ast> Visit<'ast> for RawType<'_> {
    fn visit_path(&mut self, path: &'ast syn::Path) {
        self.found |= path
            .segments
            .last()
            .is_some_and(|segment| self.names.contains(&segment.ident.to_string()));
        syn::visit::visit_path(self, path);
    }
}
fn raw_type(ty: &Type, names: &BTreeSet<String>) -> bool {
    let mut visitor = RawType {
        names,
        found: false,
    };
    visitor.visit_type(ty);
    visitor.found
}

fn raw_names(items: &[Item]) -> BTreeSet<String> {
    fn imports(tree: &UseTree, names: &mut BTreeSet<String>) {
        match tree {
            UseTree::Rename(rename) if names.contains(&rename.ident.to_string()) => {
                names.insert(rename.rename.to_string());
            }
            UseTree::Path(path) => imports(&path.tree, names),
            UseTree::Group(group) => {
                for tree in &group.items {
                    imports(tree, names);
                }
            }
            _ => {}
        }
    }
    let mut names: BTreeSet<_> = RAW_PORTS.iter().map(|name| (*name).to_owned()).collect();
    loop {
        let count = names.len();
        for item in items {
            match item {
                Item::Use(item) => imports(&item.tree, &mut names),
                Item::Type(item) if raw_type(&item.ty, &names) => {
                    names.insert(item.ident.to_string());
                }
                _ => {}
            }
        }
        if count == names.len() {
            return names;
        }
    }
}

fn borrowed_port(ty: &Type, expected: &str) -> bool {
    matches!(ty, Type::Reference(reference) if reference.mutability.is_some()
        && matches!(reference.elem.as_ref(), Type::TraitObject(object) if object.bounds.len() == 1
            && matches!(object.bounds.first(), Some(syn::TypeParamBound::Trait(bound)) if bound.path.is_ident(expected))))
}

fn allowed_raw_field(path: &str, owner: &str, field: &syn::Field) -> bool {
    let name = field.ident.as_ref().map(ToString::to_string);
    // Public ingress packages raw ports once; internal execution retains managed handles.
    let expected = match (path, owner, name.as_deref()) {
        (
            "crates/lib/application/src/use_cases/publication_run/mod.rs",
            "RunPorts",
            Some("reader"),
        ) => "ReviewReader",
        (
            "crates/lib/application/src/use_cases/publication_run/mod.rs",
            "RunPorts",
            Some("host"),
        ) => "PublicationHost",
        (
            "crates/lib/application/src/use_cases/publication_run/mod.rs",
            "RunPorts",
            Some("source"),
        ) => "CommittedSource",
        (
            "crates/lib/application/src/use_cases/publication_run/mod.rs",
            "RunPorts",
            Some("renderer"),
        ) => "ReviewRenderer",
        (
            "crates/lib/application/src/use_cases/publication_run/mod.rs",
            "RunPorts",
            Some("evidence"),
        ) => "ExecutionEvidence",
        // Cache misses reach their raw inner port through the surrounding managed facade.
        (
            "crates/lib/application/src/use_cases/publishing/checking.rs",
            "CachedSource",
            Some("inner"),
        ) => "CommittedSource",
        (
            "crates/lib/application/src/use_cases/publishing/checking.rs",
            "CachedRenderer",
            Some("inner"),
        ) => "ReviewRenderer",
        _ => return false,
    };
    borrowed_port(&field.ty, expected)
}

fn generic_raw_names(generics: &syn::Generics, names: &BTreeSet<String>) -> BTreeSet<String> {
    let raw_bound = |bounds: &syn::punctuated::Punctuated<syn::TypeParamBound, syn::Token![+]>| {
        bounds.iter().any(|bound| matches!(bound, syn::TypeParamBound::Trait(bound)
            if bound.path.segments.last().is_some_and(|segment| names.contains(&segment.ident.to_string()))))
    };
    let mut local_names = names.clone();
    for parameter in generics.type_params() {
        if raw_bound(&parameter.bounds) {
            local_names.insert(parameter.ident.to_string());
        }
    }
    if let Some(clause) = &generics.where_clause {
        for predicate in &clause.predicates {
            if let syn::WherePredicate::Type(predicate) = predicate
                && raw_bound(&predicate.bounds)
                && let Type::Path(path) = &predicate.bounded_ty
                && let Some(name) = path.path.get_ident()
            {
                local_names.insert(name.to_string());
            }
        }
    }
    local_names
}

fn storage(path: &str, items: &[Item]) -> Vec<Violation> {
    let mut violations = Vec::new();
    let names = raw_names(items);
    for item in items {
        if test_only(item_attributes(item)) {
            continue;
        }
        match item {
            Item::Struct(structure) => {
                if structure.ident == "Execution" && !structure.fields.iter().any(|field| {
                    field.ident.as_ref().is_some_and(|name| name == "session")
                        && matches!(&field.ty, Type::Path(path) if path.qself.is_none() && path.path.segments.len() == 1 && path.path.segments[0].ident == "ManagedSession")
                }) { violations.push(Violation::UnmanagedSession); }
                let local_names = generic_raw_names(&structure.generics, &names);
                for field in &structure.fields {
                    if raw_type(&field.ty, &local_names)
                        && !allowed_raw_field(path, &structure.ident.to_string(), field)
                    {
                        violations.push(Violation::RawPortField(structure.ident.to_string()));
                    }
                }
            }
            Item::Enum(enumeration) => {
                let local_names = generic_raw_names(&enumeration.generics, &names);
                for variant in &enumeration.variants {
                    for field in &variant.fields {
                        if raw_type(&field.ty, &local_names) {
                            violations.push(Violation::RawPortField(enumeration.ident.to_string()));
                        }
                    }
                }
            }
            Item::Mod(module) => {
                if let Some((_, items)) = &module.content {
                    violations.extend(storage(path, items));
                }
            }
            _ => {}
        }
    }
    violations
}

#[test]
fn production_functions_have_managed_declarations() -> Result<(), Box<dyn Error>> {
    let (root, files) = production_sources()?;
    for exemption in PURE_FILES {
        assert!(
            files.contains_key(&root.join(exemption)),
            "stale exemption {exemption}"
        );
    }
    let mut violations = Vec::new();
    for (path, file) in files {
        let relative = path.strip_prefix(&root)?.to_str().ok_or("non-UTF-8 path")?;
        for violation in inspect(relative, &file) {
            violations.push((relative.to_owned(), violation));
        }
    }
    assert_eq!(violations, []);
    Ok(())
}

#[test]
fn application_storage_uses_managed_ports() -> Result<(), Box<dyn Error>> {
    let (root, files) = production_sources()?;
    let mut execution_found = false;
    for (path, file) in files {
        let relative = path.strip_prefix(&root)?.to_str().ok_or("non-UTF-8 path")?;
        if !relative.starts_with("crates/lib/application/src/") {
            continue;
        }
        assert_eq!(storage(relative, &file.items), [], "{relative}");
        if relative.ends_with("publishing/execution.rs") {
            execution_found = file
                .items
                .iter()
                .any(|item| matches!(item, Item::Struct(item) if item.ident == "Execution"));
        }
    }
    assert!(execution_found);
    Ok(())
}

#[test]
fn undeclared_new_source_and_nested_functions_fail() -> Result<(), Box<dyn Error>> {
    let file = syn::parse_file(
        "fn work() {} impl Owner { fn method() {} } mod nested { fn hidden() {} }",
    )?;
    assert_eq!(
        inspect("new/source.rs", &file),
        [
            Violation::UndeclaredFunction("work".into()),
            Violation::UndeclaredImplementation,
            Violation::UndeclaredFunction("method".into()),
            Violation::UndeclaredFunction("hidden".into())
        ]
    );
    let file =
        syn::parse_file("#[operations] fn work() {} #[operations] impl Owner { fn method() {} }")?;
    assert_eq!(
        inspect("new/source.rs", &file),
        [
            Violation::MissingPolicy("work".into()),
            Violation::MissingPolicy("method".into())
        ]
    );
    let file = syn::parse_file("trait NewPort { fn perform(&mut self) {} }")?;
    assert_eq!(
        inspect("new/source.rs", &file),
        [Violation::UndeclaredFunction("perform".into())]
    );
    let file =
        syn::parse_file("#[observed_port] trait NewPort { #[helper] fn perform(&mut self) {} }")?;
    assert_eq!(inspect("new/source.rs", &file), []);
    Ok(())
}

#[test]
fn direct_and_aliased_lifecycle_access_fail_even_in_projection_files() -> Result<(), Box<dyn Error>>
{
    for text in [
        "use review_kit_observation::observe as run;",
        "use review_kit_observation::__private as lifecycle;",
        "use review_kit_observation::*;",
        "fn projection() { let callback = review_kit_observation::observe; }",
        "fn projection() { review_kit_observation::__private::execute(); }",
    ] {
        let file = syn::parse_file(text)?;
        assert!(!inspect(PURE_FILES[0], &file).is_empty(), "{text}");
    }
    let file = syn::parse_file(
        "use review_kit_observation::{Fields, Completion}; #[cfg(test)] mod tests { fn probe() { observe(); } }",
    )?;
    assert_eq!(inspect("new/source.rs", &file), []);
    Ok(())
}

#[test]
fn raw_storage_aliases_and_nested_handles_fail() -> Result<(), Box<dyn Error>> {
    for port in RAW_PORTS {
        let file = syn::parse_file(&format!(
            "use crate::{port} as Raw; type Alias<'a> = Box<dyn Raw + 'a>; struct State<'a> {{ port: Option<Alias<'a>> }}"
        ))?;
        assert_eq!(
            storage("new/source.rs", &file.items),
            [Violation::RawPortField("State".into())]
        );
    }
    let file = syn::parse_file(
        "mod nested { struct Execution<'a> { session: Option<Box<dyn PublicationSession + 'a>> } }",
    )?;
    assert_eq!(
        storage("new/source.rs", &file.items),
        [
            Violation::UnmanagedSession,
            Violation::RawPortField("Execution".into())
        ]
    );
    let file = syn::parse_file(
        "type Session<'a> = Box<dyn PublicationSession + 'a>; struct Execution<'a> { session: Session<'a> }",
    )?;
    assert_eq!(
        storage("new/source.rs", &file.items),
        [
            Violation::UnmanagedSession,
            Violation::RawPortField("Execution".into())
        ]
    );
    for source in [
        "struct State<S: CommittedSource> { source: S }",
        "struct State<S> where S: CommittedSource { source: S }",
        "enum State<S: CommittedSource> { Source(S) }",
        "enum State<S> where S: CommittedSource { Source(S) }",
    ] {
        let file = syn::parse_file(source)?;
        assert_eq!(
            storage("new/source.rs", &file.items),
            [Violation::RawPortField("State".into())]
        );
    }
    Ok(())
}

#[test]
fn raw_cache_exception_is_limited_to_its_field_and_file() -> Result<(), Box<dyn Error>> {
    let file = syn::parse_file("struct CachedSource<'a> { inner: &'a mut dyn CommittedSource }")?;
    assert_eq!(
        storage(
            "crates/lib/application/src/use_cases/publishing/checking.rs",
            &file.items
        ),
        []
    );
    assert_eq!(
        storage("new/source.rs", &file.items),
        [Violation::RawPortField("CachedSource".into())]
    );
    let file = syn::parse_file("struct CachedSource<'a> { other: &'a mut dyn CommittedSource }")?;
    assert_eq!(
        storage(
            "crates/lib/application/src/use_cases/publishing/checking.rs",
            &file.items
        ),
        [Violation::RawPortField("CachedSource".into())]
    );
    Ok(())
}

#[test]
fn managed_port_contracts_remain_declared() -> Result<(), Box<dyn Error>> {
    let (root, files) = production_sources()?;
    for (path, expected) in [
        (
            "ports/provider.rs",
            &["ReviewReader", "PublicationHost", "PublicationSession"][..],
        ),
        ("ports.rs", &["CommittedSource", "ReviewRenderer"][..]),
        ("use_cases/discovery_bundle/mod.rs", &["ReviewChanges"][..]),
        (
            "use_cases/initialization/mod.rs",
            &["AssessmentDraftWriter"][..],
        ),
        (
            "use_cases/publication_run/mod.rs",
            &["ExecutionEvidence"][..],
        ),
    ] {
        let file = files
            .get(&root.join("crates/lib/application/src").join(path))
            .ok_or("missing port source")?;
        for expected in expected {
            assert!(file.items.iter().any(|item| matches!(item, Item::Trait(item) if item.ident == *expected && marked(&item.attrs, "observed_port"))), "{expected}");
        }
    }
    Ok(())
}

#[derive(Default)]
struct OperationNames {
    names: BTreeSet<String>,
    errors: Vec<String>,
}

impl<'ast> Visit<'ast> for OperationNames {
    fn visit_item(&mut self, item: &'ast Item) {
        if !test_only(item_attributes(item)) {
            syn::visit::visit_item(self, item);
        }
    }

    fn visit_attribute(&mut self, attribute: &'ast Attribute) {
        if !marked(std::slice::from_ref(attribute), "operation")
            && !marked(std::slice::from_ref(attribute), "operation_alias")
        {
            return;
        }
        let result = attribute.parse_args_with(|input: syn::parse::ParseStream<'_>| {
            while !input.is_empty() {
                let key: syn::Ident = input.parse()?;
                input.parse::<syn::Token![=]>()?;
                if key == "name" {
                    self.names.insert(input.parse::<syn::LitStr>()?.value());
                } else if key == "returns" {
                    input.parse::<Type>()?;
                } else {
                    input.parse::<syn::Expr>()?;
                }
                if !input.is_empty() {
                    input.parse::<syn::Token![,]>()?;
                }
            }
            Ok(())
        });
        if let Err(error) = result {
            self.errors.push(error.to_string());
        }
    }
}

#[test]
fn declared_catalog_preserves_supported_operation_ids() -> Result<(), Box<dyn Error>> {
    // Current operation owners retain their declared identifiers.
    let expected = "
application.assessment.check application.assessment.complete application.assessment.prepare
 application.discovery application.discovery_bundle application.draft.create
application.evidence.record application.initialization
application.provider.acquire application.provider.authority application.provider.discover application.provider.readback application.provider.write
application.publication.abandon application.publication.apply application.publication.build_plan
application.publication.check_review application.publication.check_review_with_progress application.publication.plan
application.publication.plan_from_snapshot application.publication.plan_with_progress application.publication.publish
application.publication.reconcile_history application.publication.reconcile_progress application.publication.run application.publication.verify_effect
application.render.answer application.render.finding application.render.summary application.render.transition
application.source.changes application.source.lines application.source.validate_position application.summary.project
application.validation.findings application.validation.publication_shape application.validation.replies application.validation.snapshot
cli.discover cli.init cli.output cli.parse cli.render_profile.export cli.run cli.check cli.publish.run
config.connect config.load filesystem.atomic_create filesystem.atomic_write filesystem.create_bytes
filesystem.create_draft filesystem.create_json filesystem.discover_saved_context filesystem.load_context
filesystem.load_draft filesystem.load_evidence  filesystem.load_progress
filesystem.read_bytes filesystem.read_file filesystem.read_json filesystem.read_saved_changes
filesystem.record_evidence filesystem.restore_context filesystem.save_context filesystem.save_progress
filesystem.save_result filesystem.write_bytes filesystem.write_json
gitlab.acquire gitlab.authority gitlab.capture_source gitlab.check_position gitlab.connect gitlab.discover
gitlab.http.pages gitlab.http.read gitlab.http.write gitlab.prepare_write gitlab.publication.discover
gitlab.read_changes gitlab.read_diff gitlab.read_source gitlab.refresh gitlab.retained_source.decode
gitlab.retained_source.encode gitlab.retained_source.lines gitlab.retained_source.validate_position
gitlab.snapshot gitlab.source_lines gitlab.validate_plan_bodies gitlab.validate_position gitlab.validate_revision gitlab.verify_position gitlab.write
json.complete_draft json.context_bundle
 json.decode json.encode json.restore_progress json.restore_rounds
renderer.answer renderer.compile_profile renderer.export renderer.finding renderer.load
renderer.render_asset renderer.summary renderer.transition
";
    let (_, files) = production_sources()?;
    let mut observed = OperationNames::default();
    for file in files.values() {
        observed.visit_file(file);
    }
    assert!(observed.errors.is_empty(), "{:?}", observed.errors);
    let missing: Vec<_> = expected
        .split_whitespace()
        .filter(|name| !observed.names.contains(*name))
        .collect();
    assert!(missing.is_empty(), "missing operation IDs: {missing:?}");
    Ok(())
}

#[test]
fn domain_sources_and_manifest_have_no_logging_dependency() -> Result<(), Box<dyn Error>> {
    let domain = Path::new(env!("CARGO_MANIFEST_DIR")).join("../domain");
    let mut files = BTreeMap::new();
    walk_sources(&domain.join("src"), &mut files)?;
    let paths = files
        .keys()
        .cloned()
        .chain(std::iter::once(domain.join("Cargo.toml")));
    // This checks the supported dependency names, not Cargo or Rust name resolution.
    for path in paths {
        let source = fs::read_to_string(&path)?;
        for dependency in [
            "review-kit-observation",
            "review_kit_observation",
            "slog",
            "redactable",
        ] {
            assert!(
                !source.contains(dependency),
                "{} contains {dependency}",
                path.display()
            );
        }
    }
    Ok(())
}
