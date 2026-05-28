# Contributing to VDS-MCP

Thank you for your interest in contributing to the project! This guide will help you get started with development,
understand our workflow, and make meaningful contributions.

## Table of Contents

- [Code of Conduct](#code-of-conduct)
- [Getting Started](#getting-started)
- [Development Environment](#development-environment)
- [Project Structure](#project-structure)
- [Development Workflow](#development-workflow)
- [Coding Standards](#coding-standards)
- [Testing Guidelines](#testing-guidelines)
- [Documentation](#documentation)
- [Pull Request Process](#pull-request-process)
- [Release Process](#release-process)
- [Getting Help](#getting-help)

---

## Code of Conduct

We are committed to providing a welcoming and inclusive environment. All contributors are expected to:

- Be respectful and considerate
- Welcome newcomers and help them get started
- Focus on constructive feedback
- Assume good intentions
- Respect differing viewpoints and experiences

---

## Getting Started

### Prerequisites

- **Rust:** 1.85 or later (2024 edition)
- **Git:** For version control
- **IDE:** VS Code with rust-analyzer recommended
- **OS:** Windows, Linux, or macOS
- **cargo-nextest:** For running tests (`cargo install cargo-nextest`)

### Quick Start

1. **Fork and Clone**
   ```bash
   git clone https://github.com/huhlig/vds-mcp.git
   cd bdrpc
   ```

2. **Build the Project**
   ```bash
   cargo build
   ```

3. **Run Tests**
   ```bash
   cargo nextest run
   ```

4. **Run Benchmarks**
   ```bash
   cargo bench
   ```

---

## Development Environment

### Required Tools

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install additional components
rustup component add rustfmt clippy

# Install cargo tools
cargo install cargo-nextest cargo-watch cargo-audit cargo-outdated
```

### Recommended VS Code Extensions

- **rust-analyzer:** Rust language support
- **CodeLLDB:** Debugging support
- **Even Better TOML:** TOML file support
- **Error Lens:** Inline error display
- **GitLens:** Git integration

### Environment Configuration

Create a `.env` file in the project root (optional):

```bash
RUST_LOG=debug
RUST_BACKTRACE=1
```

---

## Project Structure

```
vds-mcp/
├── benches/               # Service Benchmakrs 
│   └── benchmark.rs
├── docs/                  # Project Documentation
│   ├── installation.md    # Command Line Wrapper
│   └── overview.md
├── src/
│   ├── bin.rs             # Command Line Wrapper
│   ├── document.rs        # Core Document Model
│   ├── lib.rs             # Main Library File
│   ├── markdown.rs        # Markdown Utilities
│   ├── mcp.rs             # MCP Facade
│   ├── service.rs         # MCP Service
│   └── storage.rs         # Storage Backend
├── tests/
│   ├── mcp_smoke.rs       # MCP Smoke Tests
│   └── overview.rs        # Integration Tests
├── AGENTS.md              # VDS Agent Usage Instructions
├── Cargo.toml             # Workspace configuration
├── CONTRIBUTING.md        # This file
├── LICENSE.md             # Apache2 License
└── README.md              # Project Readme.
```

### Module Organization

**Key Principles:**

- Each crate has a single, well-defined responsibility
- Dependencies flow downward (no circular dependencies)
- Core transport and serialization layers are independent
- Public APIs are clearly separated from internal implementation
- No `mod.rs` files - use modern Rust module structure

---

## Development Workflow

### Branch Strategy

- **main:** Stable, production-ready code
- **develop:** Integration branch for features
- **feature/\*:** Feature development branches
- **fix/\*:** Bug fix branches
- **docs/\*:** Documentation updates

### Creating a Feature Branch

```bash
git checkout develop
git pull origin develop
git checkout -b feature/your-feature-name
```

### Commit Message Format

Follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:

```
<type>(<scope>): <subject>

<body>

<footer>
```

**Types:**

- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `style`: Code style changes (formatting, etc.)
- `refactor`: Code refactoring
- `perf`: Performance improvements
- `test`: Adding or updating tests
- `chore`: Maintenance tasks

**Examples:**

```
feat(transport): implement TCP transport with reconnection

Add TCP transport implementation with exponential backoff
reconnection strategy and configurable timeouts.

Closes #123
```

```
fix(serialization): correct frame length validation

The previous implementation didn't handle edge cases for
maximum frame size. This fix ensures proper validation.

Fixes #456
```

### Daily Development Cycle

1. **Pull latest changes**
   ```bash
   git checkout develop
   git pull origin develop
   ```

2. **Create/update feature branch**
   ```bash
   git checkout -b feature/my-feature
   ```

3. **Make changes and test**
   ```bash
   # Make your changes
   cargo fmt
   cargo clippy
   cargo nextest run
   ```

4. **Commit changes**
   ```bash
   git add .
   git commit -m "feat(module): description"
   ```

5. **Push and create PR**
   ```bash
   git push origin feature/my-feature
   # Create PR on GitHub
   ```

---

## Coding Standards

### Rust Style Guide

We follow the [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) and enforce style with `rustfmt`.

**Key Conventions:**

1. **Naming:**
    - Types: `PascalCase`
    - Functions/variables: `snake_case`
    - Constants: `SCREAMING_SNAKE_CASE`
    - Lifetimes: `'a`, `'b`, etc.

2. **Error Handling:**
    - Use `Result<T, E>` for recoverable errors
    - Use `panic!` only for unrecoverable errors
    - Provide context with error types
    - Use `thiserror` for error definitions

3. **Documentation:**
    - All public items must have doc comments
    - Include examples in doc comments
    - Document panics, errors, and safety requirements

4. **Safety:**
    - Minimize `unsafe` code
    - Document all `unsafe` blocks with safety invariants
    - Prefer safe abstractions

5. **Modern Rust:**
    - Use Rust 2024 edition features
    - No `mod.rs` files - use modern module structure
    - Leverage async/await for asynchronous operations

### Code Formatting

```bash
# Format all code
cargo fmt

# Check formatting without modifying
cargo fmt -- --check
```

### Linting

```bash
# Run clippy
cargo clippy -- -D warnings

# Run clippy with all features
cargo clippy --all-features -- -D warnings
```

### Example Code Style

```rust
/// Sends a message through the channel.
///
/// # Arguments
///
/// * `message` - The message to send (must be serializable)
///
/// # Returns
///
/// Returns `Ok(())` on success, or an error if the send fails.
///
/// # Errors
///
/// Returns `ChannelError::Closed` if the channel is closed.
/// Returns `ChannelError::SerializationError` if serialization fails.
///
/// # Examples
///
/// ```
/// use bdrpc::Channel;
///
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// let channel = Channel::new()?;
/// channel.send(&"Hello, world!").await?;
/// # Ok(())
/// # }
/// ```
pub async fn send<T: Serialize>(&self, message: &T) -> Result<(), ChannelError> {
    if self.is_closed() {
        return Err(ChannelError::Closed);
    }

    let serialized = self.serializer.serialize(message)?;
    self.transport.send(serialized).await?;

    Ok(())
}
```

---

## Testing Guidelines

### Test Organization

```
bdrpc/
├── src/
│   ├── lib.rs
│   ├── module.rs
│   └── module/
│       ├── implementation.rs
│       └── tests.rs          # Unit tests
├── tests/
│   ├── integration_test.rs   # Integration tests
│   └── common/
│       └── mod.rs            # Test utilities
└── benches/
    └── benchmark.rs          # Benchmarks
```

### Unit Tests

- Test each function in isolation
- Cover edge cases and error paths
- Use descriptive test names
- Use `cargo nextest run` for running tests

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_send_and_receive_success() {
        let (tx, rx) = create_channel().await.unwrap();
        tx.send(&"test message").await.unwrap();
        let received: String = rx.receive().await.unwrap();
        assert_eq!(received, "test message");
    }

    #[tokio::test]
    async fn test_send_on_closed_channel_returns_error() {
        let (tx, _rx) = create_channel().await.unwrap();
        drop(_rx);
        assert!(tx.send(&"test").await.is_err());
    }

    #[tokio::test]
    async fn test_receive_on_empty_channel_waits() {
        let (_tx, rx) = create_channel().await.unwrap();
        let timeout = tokio::time::timeout(
            Duration::from_millis(100),
            rx.receive::<String>()
        );
        assert!(timeout.await.is_err());
    }
}
```

### Integration Tests

- Test multi-component interactions
- Test end-to-end workflows
- Use realistic scenarios

```rust
// tests/integration_test.rs
use bdrpc::{Endpoint, TcpTransport, JsonSerializer};

#[tokio::test]
async fn test_bidirectional_communication() {
    let server = Endpoint::bind("127.0.0.1:0")
        .with_transport(TcpTransport::new())
        .with_serializer(JsonSerializer::new())
        .build()
        .await
        .unwrap();

    let client = Endpoint::connect(server.local_addr())
        .with_transport(TcpTransport::new())
        .with_serializer(JsonSerializer::new())
        .build()
        .await
        .unwrap();

    // Test bidirectional communication
    client.send(&"ping").await.unwrap();
    let msg: String = server.receive().await.unwrap();
    assert_eq!(msg, "ping");

    server.send(&"pong").await.unwrap();
    let msg: String = client.receive().await.unwrap();
    assert_eq!(msg, "pong");
}
```

### Property-Based Tests

Use `proptest` for property-based testing:

```rust
use proptest::prelude::*;

proptest! {
    #[test]
    fn test_serialization_roundtrip(
        data in prop::collection::vec(any::<u8>(), 0..1000)
    ) {
        let serializer = PostcardSerializer::new();
        let serialized = serializer.serialize(&data).unwrap();
        let deserialized: Vec<u8> = serializer.deserialize(&serialized).unwrap();
        prop_assert_eq!(data, deserialized);
    }
}
```

### Benchmarks

Use `criterion` for benchmarks:

```rust
use criterion::{criterion_group, criterion_main, Criterion};

fn benchmark_serialization(c: &mut Criterion) {
    let serializer = PostcardSerializer::new();
    let data = vec![0u8; 1024];

    c.bench_function("serialize 1KB", |b| {
        b.iter(|| {
            serializer.serialize(std::hint::black_box(&data)).unwrap();
        });
    });
}

criterion_group!(benches, benchmark_serialization);
criterion_main!(benches);
```

### Running Tests

```bash
# Run all tests with nextest
cargo nextest run

# Run specific test
cargo nextest run test_name

# Run with output
cargo nextest run -- --nocapture

# Run benchmarks
cargo bench

# Run with coverage (requires tarpaulin)
cargo tarpaulin --out Html
```

### Test Coverage Goals

- **Unit tests:** 90%+ coverage
- **Integration tests:** All major workflows
- **Property tests:** Core algorithms
- **Benchmarks:** Performance-critical paths

---

## Documentation

### Documentation Types

1. **Code Documentation:** Inline doc comments
2. **ADRs:** Architecture Decision Records (docs/ADR/)
3. **Guides:** User and developer guides (docs/)
4. **API Reference:** Generated from doc comments

### Writing Doc Comments

```rust
/// Brief one-line summary.
///
/// More detailed description with multiple paragraphs if needed.
/// Explain the purpose, behavior, and any important details.
///
/// # Arguments
///
/// * `param1` - Description of first parameter
/// * `param2` - Description of second parameter
///
/// # Returns
///
/// Description of return value.
///
/// # Errors
///
/// List possible error conditions.
///
/// # Panics
///
/// Describe panic conditions if any.
///
/// # Safety
///
/// Document safety requirements for unsafe functions.
///
/// # Examples
///
/// ```
/// use bdrpc::example;
///
/// # async fn test() -> Result<(), Box<dyn std::error::Error>> {
/// let result = example(42).await?;
/// assert_eq!(result, 84);
/// # Ok(())
/// # }
/// ```
pub async fn example(param: i32) -> Result<i32, Error> {
    // Implementation
}
```

### Generating Documentation

```bash
# Generate and open documentation
cargo doc --open

# Generate with private items
cargo doc --document-private-items
```

### Creating ADRs

When making significant architectural decisions:

1. Copy an existing ADR template from `docs/ADR/`
2. Number sequentially (ADR-XXX)
3. Fill in all sections
4. Submit as part of your PR

---

## Pull Request Process

### Before Submitting

- [ ] Code compiles without warnings
- [ ] All tests pass (`cargo nextest run`)
- [ ] Code is formatted (`cargo fmt`)
- [ ] Clippy passes (`cargo clippy`)
- [ ] Documentation is updated
- [ ] Examples are updated if API changed
- [ ] Commit messages follow conventions
- [ ] Branch is up to date with develop

### PR Template

```markdown
## Description

Brief description of changes.

## Type of Change

- [ ] Bug fix
- [ ] New feature
- [ ] Breaking change
- [ ] Documentation update

## Related Issues

Closes #123

## Testing

Describe testing performed:

- Unit tests added/updated
- Integration tests added/updated
- Manual testing performed

## Checklist

- [ ] Code follows style guidelines
- [ ] Self-review completed
- [ ] Documentation updated
- [ ] Examples updated
- [ ] Tests added/updated
- [ ] All tests pass
- [ ] No new warnings
```

### Review Process

1. **Automated Checks:** CI must pass
2. **Code Review:** At least one approval required
3. **Testing:** Reviewer verifies tests are adequate
4. **Documentation:** Reviewer checks docs are updated
5. **Merge:** Squash and merge to develop

### Review Guidelines

**For Authors:**

- Keep PRs focused and reasonably sized
- Respond to feedback promptly
- Be open to suggestions
- Update PR based on feedback

**For Reviewers:**

- Review within 2 business days
- Be constructive and specific
- Ask questions if unclear
- Approve when satisfied

---

## Release Process

### Version Numbers

We follow [Semantic Versioning](https://semver.org/):

- **MAJOR:** Breaking changes
- **MINOR:** New features (backward compatible)
- **PATCH:** Bug fixes (backward compatible)

### Release Checklist

1. Update version in `Cargo.toml` files
2. Update `CHANGELOG.md`
3. Run full test suite
4. Create release branch
5. Tag release
6. Publish to crates.io
7. Create GitHub release
8. Update documentation site

---

## Getting Help

### Resources

- **Documentation:** [docs/](docs/)
- **ADRs:** [docs/ADR/](docs/ADR/)
- **Post-v0.1.0 Roadmap:** [docs/dev/post-v0.1.0-roadmap.md](docs/dev/post-v0.1.0-roadmap.md)
- **v0.1.0 Implementation Plan (Archived):
  ** [docs/dev/archive/implementation-plan-v0.1.0.md](docs/dev/archive/implementation-plan-v0.1.0.md)
- **Open Questions:** [docs/dev/open-questions.md](docs/dev/open-questions.md)

### Communication Channels

- **GitHub Issues:** Bug reports and feature requests
- **GitHub Discussions:** Questions and general discussion
- **Pull Requests:** Code review and collaboration

### Asking Questions

When asking for help:

1. Search existing issues/discussions first
2. Provide context and details
3. Include code examples if relevant
4. Describe what you've tried
5. Be patient and respectful

### Channel Creation Guidelines

When creating channels in BDRPC, follow these guidelines:

#### In-Memory Channels

Use `Channel::new_in_memory()` for:

- **Unit tests** - Testing channel logic in isolation
- **Examples** - Demonstrating channel API without network complexity
- **In-process communication** - When you need typed message passing within a single process

```rust
use bdrpc::channel::{Channel, ChannelId};

// Create in-memory channel
let (sender, receiver) = Channel::<MyProtocol>::new_in_memory(ChannelId::new(), 100);
```

**Important:** In-memory channels are NOT connected to any transport and cannot communicate across network boundaries.

#### Network Channels

Use the Endpoint API for:

- **Production applications** - Real network communication
- **Client-server systems** - TCP/UDP communication
- **Distributed systems** - Multi-node communication
- **Integration tests** - Testing full network stack

```rust
use bdrpc::endpoint::{Endpoint, EndpointConfig};
use bdrpc::serialization::JsonSerializer;

// Create endpoint
let mut endpoint = Endpoint::new(JsonSerializer::default (), EndpointConfig::default ());

// Register protocol
endpoint.register_bidirectional("MyProtocol", 1).await?;

// Connect and create channel
let connection = endpoint.connect("127.0.0.1:8080").await?;
let sender = endpoint.channel_manager()
.create_channel::<MyProtocol>(ChannelId::new(), 100)
.await?;
```

#### Example Reference

- `channel_basics.rs` - In-memory channels only
- `advanced_channels.rs` - In-memory channel management
- `chat_server.rs` - In-memory multi-client pattern
- `calculator.rs` - TCP transport + in-memory channels (demo)
- `network_chat.rs` - **Full Endpoint API with network channels** (recommended pattern)

#### Migration Path

If you have code using the deprecated `Channel::new()`:

1. For tests/examples: Change to `Channel::new_in_memory()`
2. For production: Migrate to Endpoint API (see `network_chat.rs`)
3. See `MIGRATION.md` for detailed migration guide

---

## Additional Guidelines

### Performance Considerations

- Profile before optimizing
- Document performance-critical code
- Add benchmarks for hot paths
- Consider memory allocation patterns
- Use appropriate data structures
- Leverage async/await efficiently

### Security Considerations

- Validate all inputs
- Handle errors securely
- Avoid information leaks
- Document security assumptions
- Report security issues privately

### Backward Compatibility

- Maintain API stability
- Deprecate before removing
- Provide migration guides
- Version protocol formats
- Test upgrade paths

---

## Recognition

Contributors are recognized in:

- Git commit history
- Release notes
- Project README
- Annual contributor list

Thank you for contributing to BDRPC! 🚀

---

## License

By contributing to BDRPC, you agree that your contributions will be licensed under the same license as the project (
see [LICENSE.md](LICENSE.md)).