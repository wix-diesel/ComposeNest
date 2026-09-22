# AGENTS.md

## Project Overview

This project is a cross-platform desktop application that helps users create and manage Docker environments through a graphical user interface.

The application aims to make Docker easier to use, especially for users who are not familiar with Docker Compose configuration.

The application targets:

* Windows
* macOS
* Linux

The primary technology stack is:

* Rust
* Tauri
* React
* TypeScript
* Vite

The v1 requirements and architecture are documented in `docs/requirements-v1.md` and `docs/architecture-v1.md`. Follow their adopted decisions and distinguish them from pending implementation and validation work.

Do not introduce major architectural decisions unless they are explicitly required by the current task or specification.

## General Coding Principles

Prioritize readability and maintainability.

Prefer simple and explicit code over clever or overly abstract implementations.

Keep changes focused on the requested task.

Avoid premature abstraction and unnecessary generalization.

Do not introduce new architectural patterns solely for hypothetical future requirements.

Follow existing project conventions when they have already been established.

## Functions and Methods

Keep functions and methods small and focused.

A function or method should ideally have one clear responsibility.

Avoid long functions containing multiple unrelated operations.

When logic becomes difficult to understand as a single unit, extract meaningful functions.

Prefer descriptive names that explain intent.

Good:

```rust
validate_port(...)
generate_instance_name(...)
check_docker_status(...)
```

Avoid names that hide multiple responsibilities:

```rust
process_everything(...)
handle_data(...)
do_work(...)
```

Do not split code into excessively small functions when doing so makes the control flow harder to understand.

The goal is readability, not minimizing line count.

## Types and Classes

Keep structs, enums, classes, interfaces, and components focused.

Avoid types that accumulate unrelated responsibilities.

Prefer meaningful domain-specific names.

Avoid vague names such as:

* `Manager`
* `Helper`
* `Utils`
* `Common`
* `Misc`

unless the responsibility is genuinely clear from the context.

Avoid excessively large source files.

Split files when doing so creates clear and meaningful boundaries.

## Documentation

Code comments and API documentation must be written in English.

This applies to:

* Rust documentation comments
* Rust inline comments
* TypeScript JSDoc
* TypeScript inline comments
* React comments
* comments in configuration or source files when they explain implementation behavior

Public APIs should have concise documentation explaining their purpose.

In Rust, document public:

* structs
* enums
* traits
* functions
* methods
* important public fields

Example:

```rust
/// Represents a Docker environment managed by the application.
pub struct Instance {
    /// Unique identifier of the instance.
    pub id: String,

    /// Display name shown to the user.
    pub name: String,
}
```

Public functions and methods should describe their observable behavior.

Example:

```rust
/// Returns whether the specified host port is currently available.
pub fn is_port_available(port: u16) -> bool {
    // ...
}
```

Avoid comments that merely repeat the code.

Bad:

```rust
/// Gets the name.
pub fn name(&self) -> &str {
    &self.name
}
```

Add documentation when it provides useful context, intent, constraints, or behavior.

For TypeScript, use concise JSDoc for exported APIs and public class properties where useful.

Example:

```ts
/** Represents an item displayed in the container list. */
export interface ContainerItem {
  /** Stable identifier of the item. */
  id: string;

  /** Human-readable display name. */
  name: string;
}
```

Internal comments should explain why something is done when the reason is not obvious.

Do not add comments for self-explanatory implementation details.

## GitHub Communication

GitHub communication must be written in Japanese.

This applies to:

* Issue titles
* Issue descriptions
* Pull Request titles
* Pull Request descriptions
* review comments
* replies to review comments
* discussion comments related to Issues or Pull Requests

Use clear and concise Japanese suitable for technical collaboration.

Technical identifiers such as the following may remain in English when appropriate:

* class names
* function names
* type names
* command names
* file names
* API names
* library names
* protocol names
* error messages that must be quoted exactly

Example Issue title:

```text
PostgreSQL テンプレートからインスタンスを作成できるようにする
```

Example Pull Request title:

```text
PostgreSQL インスタンス作成機能を追加
```

Example review reply:

```text
ご指摘ありがとうございます。
Port の重複チェックを作成前に実行するよう修正しました。
あわせて回帰テストを追加しています。
```

Do not translate source-code comments into Japanese.

Do not write Issue or Pull Request descriptions in English unless the user explicitly requests it.

## Rust Guidelines

Write idiomatic Rust.

Prefer:

* strong typing
* enums over loosely related string constants
* immutable values by default
* explicit error handling
* small and focused modules

Use `Result` for fallible operations.

Avoid `unwrap()` and `expect()` in production code unless failure is demonstrably impossible.

If `unwrap()` or `expect()` is necessary, the reason should be clear from the surrounding code or documented when not obvious.

Prefer meaningful error types and messages.

Avoid unnecessary cloning.

Avoid unsafe Rust unless there is a strong technical reason for using it.

If `unsafe` is required, document why it is necessary and what invariants must be maintained.

Run standard Rust formatting and linting tools.

Code should pass:

```text
cargo fmt
cargo clippy
cargo test
```

unless a task explicitly requires otherwise.

Address meaningful Clippy warnings rather than suppressing them without justification.

## TypeScript Guidelines

Use TypeScript strictly.

Avoid `any` unless integration with an external API makes it unavoidable.

Prefer explicit types at public boundaries.

Use type inference where the resulting type remains obvious.

Prefer:

* interfaces or types with clear responsibilities
* discriminated unions where appropriate
* immutable values where practical
* descriptive variable and function names

Avoid large functions and deeply nested conditional logic.

Do not suppress TypeScript errors without a clear reason.

## React Guidelines

Use functional React components.

Prefer hooks over class components.

Keep components reasonably small and focused.

Extract components when a section has its own meaningful responsibility.

Avoid components that contain large amounts of unrelated UI and logic.

Prefer clear data flow over implicit side effects.

Keep state as local as practical.

Do not introduce global state management libraries unless there is a demonstrated need.

Avoid unnecessary `useEffect` usage.

Do not optimize with `useMemo`, `useCallback`, or memoization unless there is a concrete reason.

Prioritize understandable React code over premature performance optimization.

## Tauri Guidelines

Use Tauri APIs according to current official practices.

Keep Rust and TypeScript interfaces strongly typed where practical.

Do not expose arbitrary shell execution or unrestricted operating-system access to the frontend.

Only expose capabilities required by the application.

Validate externally supplied values before using them in operating-system operations.

Detailed Tauri application structure should follow the architecture decided during the design phase rather than being assumed in advance.

## External Process Execution

Treat external process execution as a security-sensitive operation.

Never build shell commands by concatenating untrusted input.

Prefer APIs that pass executable names and arguments separately.

For example, prefer the conceptual equivalent of:

```text
command("docker")
args(["compose", "up", "-d"])
```

over:

```text
shell("docker compose up -d " + user_input)
```

Check:

* process exit code
* standard output
* standard error

when relevant.

Provide meaningful error information when external commands fail.

## Security

Treat Docker-related operations and local system access as privileged operations.

Validate:

* paths
* ports
* identifiers
* user input
* configuration values
* external command arguments

Do not log:

* passwords
* tokens
* secrets
* credentials

Do not expose sensitive values to the frontend unless required for the requested operation.

Prefer least-privilege access where practical.

## Error Handling

Handle expected failures explicitly.

Do not silently ignore errors.

User-facing errors should be understandable without requiring knowledge of Rust or Docker internals.

Where appropriate, preserve technical details separately for diagnostics.

Avoid exposing raw stack traces or internal implementation details as the primary user-facing error.

## Testing

Add tests for meaningful behavior.

Prefer small, focused tests.

When fixing a bug, add a regression test when practical.

Tests should clearly communicate:

* what behavior is being tested
* the relevant input
* the expected result

Avoid tests that depend unnecessarily on execution order or shared mutable state.

Do not require external infrastructure for tests that do not actually need it.

Integration tests that require Docker or another external dependency should be clearly distinguishable from normal unit tests.

## Dependencies

Avoid adding dependencies without a clear benefit.

Before introducing a new dependency, consider:

* maintenance activity
* license
* platform support
* security history
* transitive dependencies
* whether existing dependencies or the standard library already provide the required functionality

Prefer mature and actively maintained libraries.

Do not add a large dependency for trivial functionality.

## Changes

When modifying existing code:

1. Understand the existing behavior first.
2. Make the smallest coherent change that satisfies the requirement.
3. Avoid unrelated refactoring.
4. Add or update relevant tests.
5. Update public documentation when public behavior changes.
6. Keep Windows, macOS, and Linux compatibility in mind.

Do not silently change existing public behavior unless the task requires it.

## Cross-Platform Development

The application must support:

* Windows
* macOS
* Linux

Do not assume Unix-only behavior.

Be careful with:

* filesystem paths
* path separators
* executable locations
* process behavior
* line endings
* permissions
* environment variables
* operating-system-specific APIs

Use cross-platform Rust and Tauri APIs where practical.

Any intentional platform-specific implementation should be isolated and clearly documented.

## Definition of Done

A change is complete when:

* the requested behavior is implemented
* the code is readable
* functions and methods remain focused
* types do not accumulate unrelated responsibilities
* public APIs have concise English documentation where appropriate
* public properties and fields have concise English documentation where appropriate
* source-code comments are written in English
* GitHub Issues and Pull Requests are written in Japanese
* errors are handled appropriately
* relevant tests exist and pass
* formatting passes
* lint checks pass
* unnecessary warnings are not introduced
* Windows, macOS, and Linux compatibility has been considered
* no unrelated architectural decisions have been introduced

## Guiding Principle

Write code that another developer can understand without excessive context.

Prefer clarity over cleverness.

Keep public APIs documented, functions focused, types manageable, and implementation decisions proportional to the current requirements.

Architecture and detailed application design should be determined separately during the design phase.
