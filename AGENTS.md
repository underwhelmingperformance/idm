# Agent rules

This is going to be a Rust program to interact with "idotmatrix" LED dot matrix
devices. These communicate over BLE.

## General

- Write in British English.
- Always use `cargo add` to add dependencies, never edit `Cargo.toml` manually.
- When implementing new handlers or protocol behaviour, read `docs/handlers.md`
  and `docs/protocol.md` to get yourself oriented.
- After finishing a piece of work on a handler, update `docs/handlers.md`
  accordingly. It's not a running log of work. Just update the status and add
  any notes regarding left-over work.

## Committing

- Use conventional commits.
- Write clear commit messages that explain the "why" behind the change, not just
  the "what". Write as a professional principal software engineer, not an AI.
- It's usually paragraph:
  1. Set the scene:
     > We've got a problem with our foo system at the minute. When we do bar, we
     > see baz, which is not what we want. This is because of quux.
  2. Explain the problem:
     > This happens because of x, y, z.
  3. Explain the solution:
     > To fix this, we need to do a, b, c. This will allow us to do d, e, f,
     > which will solve the problem.
- Those are not strict rules. You don't have to conform to exactly 3 paragraphs
  if it doesn't make sense in a particular case.
- Wrap at 72 characters in commit messages.
- No `Co-authored-by`.

## Rust style guidelines

- Follow Rust's idioms and best practices.
- Be type-first: prefer to create types with associated methods rather than free
  functions.
- Where possible, implement Rust traits instead of doing things ad-hoc, e.g.,
  `From`, `Display`, `Error`, etc.
- No program logic in `mod.rs` or `lib.rs` files: these are only for module
  declarations and re-exports.
- We have strict clippy settings. NEVER use `allow` or `deny` attributes to
  silence clippy warnings. Instead, fix the underlying issue.
- Consider the truly public API surface carefully. Only expose what is necessary
  and use `pub(crate)` or private visibility for everything else.

## Libraries

- BLE: use `btleplug`.

## CLI

- Use `clap` for command line argument parsing.
- Always parse straight to proper types.

## Errors

- Use `thiserror` for defining error types.
  - Have variants for different error cases.
  - Use `#[from]` for error conversions.
  - `map_err` should not be needed.
- Use `anyhow` for error handling in application code.

## o11y

- Use `tracing` and `tracing-opentelemetry`.
- Add traces and spans throughout.
- Use events for significant occurrences.
- Log at appropriate levels.
- If outputting to a terminal, use a pretty formatter. Otherwise, use JSON.

## Testing

- We must have unit and integration tests.
- Use fake services for the integration testsuite.
- Always use `pretty_assertions::assert_eq!` for equality assertions.
- Avoid writing repetitive tests: use parameterised tests instead. Use `rstest`
  for this.
- Use dependency injection via traits to make code testable.
- Make sure to run the tests frequently during development.
- Don't use `unwrap()` in tests: use `?` or `expect()` with a helpful message.
- Use structural assertions on full objects. Our output should be deterministic
  so this ought to be possible. Don't repeatedly assert on the same value in
  tests: assign it to a variable instead. Instead of:

  ```rust
   assert_eq!(1, foo.bar);
   assert_eq!(2, foo.baz);
   assert_eq!(3, foo.quux);
  ```

  do:

  ```rust
  let bar = foo.bar();
  assert_eq(Foo{
      bar: 1,
      baz: 2,
      quux: 3,
  }, foo);
  ```

- When working with slices, don't compare the length and then look at some
  items. Instead, write out the whole expected slice and compare that.
- Use the `assert_matches` crate for matching error variants and other sum types
  in tests.

## Documentation

- All public items must have doc comments.
- Write documentation as if you are a professional principal software engineer,
  not an AI.
- Include doctests for all public functions and methods.
