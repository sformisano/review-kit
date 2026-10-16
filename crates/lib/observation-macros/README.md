# review-kit-observation-macros

This crate generates synchronous operation entry points and managed port facades.
The [observation crate](../observation/README.md#generated-operation-boundaries) reexports its macros and owns runtime logging policy.

## Operation declarations

Apply `#[review_kit_observation::operations]` to a function, inline module or implementation block.
Every function requires one declaration:

- `#[operation(name = "fixed.identity", input = fields_expression, output = completion_policy)]`
- `#[helper]` for an exempt helper or compatibility binder.

The input expression produces `Fields`; the completion policy borrows the typed result and produces `Completion`.
Keep completion policies based on that result so they do not compete with the body's mutable borrows.
The generator preserves the function signature and nests its body inside the runtime call.
Generic functions and borrowed outputs retain their Rust types.
Function declarations stay in their original lexical module. Completion closures receive the typed result without requiring redundant parameter annotations.
Early returns and `?` terminate the nested business body, then complete observation.

Managed modules include inline child modules and implementation blocks.
Out-of-line child modules and item macros are rejected because their callable surface cannot be checked within that declaration.
Managed functions must be synchronous, safe Rust functions without `const`, foreign ABI or variadic arguments.
Observed functions reject `track_caller` because the generated closure changes its caller location.
They also reject `cfg_attr`, which could enable unsupported attributes conditionally.
Helper declarations preserve these attributes.

## Provider declarations

`#[review_kit_observation::observed_port(wrapper = Name, ownership = "borrowed", visibility = "pub(crate)")]` generates a raw trait and a facade.
`ownership = "boxed"` stores an existing boxed implementation with its original lifetime.
The facade's raw field resides in a private generated module.
Each raw trait method requires an operation policy or a helper exemption.
Facade methods forward default methods as well as required methods.

The supported provider traits have no type parameters, associated items or generic methods, and use `&self` or `&mut self` receivers.
The compiler rejects unsupported forms.
Facade methods live in a generated child module, so copied signatures, attributes, policies and the `reader` option reject `super::` paths.
Use imported names or `crate::` paths. `self::` paths and literal strings remain supported.
Raw default-method bodies retain their original module and may use `super::`.
The boxed publication session supports one reader supertrait through `reader = ManagedReader`.
That option exposes another managed handle, retaining private access to the raw implementation.
Generic application functions remain supported. The port generator does not support associated types.

Two method options preserve publication contracts:

- `returns = Type, map = mapper` changes only the facade's return type, wrapping an acquired raw session immediately. The mapper is business adaptation and its failures are not swallowed as logging failures.
- `#[operation_alias(method = readback, name = "...", input = ..., output = ...)]` generates a separately named observed method that delegates to the same raw method. Discovery and readback use this distinction.

The generated facade does not implement the raw trait.
Internal composition reuses managed handles; it does not feed them back into public raw-port binders.
This avoids duplicate records for the same boundary.

## Runtime boundary and verification

Generated code calls the observation crate's public hidden `__private` bridge across crate boundaries.
The bridge always enters the shared lifecycle; it exposes no raw operation body.
Its input projection helper catches unwinding failures and skips projection when no observer is installed.
Application code uses the generated operation API rather than this bridge.

Runtime and compile-fail tests live in `review-kit-observation` to exercise the generated code through its actual consumer.
Application tests cover custom readers, borrowed sessions, exact operation counts, readback correlation, redaction and unchanged publication behavior.
The application architecture tests discover production source files across application, infrastructure and CLI, check operation declarations and reject direct runtime entry.
They also check stored raw ports against explicit ingress and cache ownership exceptions.
They supplement Rust privacy and required metadata; they do not infer whether an arbitrary helper deserves a separate operation.
