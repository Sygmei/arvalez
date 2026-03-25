# arvalez

Modern OpenAPI client generator with a compiler-style core, typed IR, and WASM plugins.

## Current Scope

This first implementation slice focuses on the plugin architecture and the first target backends:

- shared IR types in Rust
- a plugin SDK that reads and writes IR over a serde boundary
- a WASM runtime that executes plugins as WASI programs
- a sample `money` plugin that rewrites decimal money fields to a domain type
- a first Python backend that emits Pydantic models plus sync and async `httpx` clients
- a first TypeScript backend that emits typed models plus a `fetch`-based client
- a first Go backend that emits typed models plus a `net/http` client

The OpenAPI importer is in place, and the generator can now emit Python and TypeScript packages from `openapi.json`. Go is still to come.

## Workspace Layout

- `crates/arvalez-ir`: shared IR and validation
- `crates/arvalez-openapi`: OpenAPI document loader and Core IR mapper
- `crates/arvalez-plugin-sdk`: plugin-side protocol helpers
- `crates/arvalez-plugin-runtime`: host-side WASM execution
- `crates/arvalez-target-python`: Python SDK generator
- `crates/arvalez-target-typescript`: TypeScript SDK generator
- `crates/arvalez-cli`: local CLI for inspecting IR and running plugins
- `plugins/money-plugin`: example WASM plugin

## Build

Install the WASI target if needed:

```bash
rustup target add wasm32-wasip1
```

Build the example plugin:

```bash
cargo build -p money-plugin --target wasm32-wasip1
```

Run the plugin against the fixture IR:

```bash
cargo run -p arvalez-cli -- run-plugin --plugin money
```

Build Core IR from the local OpenAPI document:

```bash
cargo run -p arvalez-cli -- build-ir --openapi openapi.json
```

Run the plugin against a real OpenAPI document:

```bash
cargo run -p arvalez-cli -- run-plugin --plugin money --openapi openapi.json
```

Generate a Python SDK package:

```bash
cargo run -p arvalez-cli -- generate-python --openapi openapi.json --output generated/python-client
```

Generate a TypeScript SDK package:

```bash
cargo run -p arvalez-cli -- generate-typescript --openapi openapi.json --output generated/typescript-client
```

Generate a Go SDK package:

```bash
cargo run -p arvalez-cli -- generate-go --openapi openapi.json --output generated/go-client
```

The generators also read optional settings from `arvalez.toml`:

```toml
[generator]
group_by_tag = true
version = "1.0.0"

[generator.go]
module_path = "github.com/acme/client"
package_name = "client"
version = "1.0.0"

[generator.python]
package_name = "arvalez_client"
version = "1.0.1"
template_dir = "./templates/python"

[generator.typescript]
package_name = "@arvalez/client"
version = "1.0.2"
template_dir = "./templates/typescript"
```

Generate all enabled backends into one output root:

```bash
cargo run -p arvalez-cli -- generate --openapi openapi.json --output-root generated
```

Disable a backend from the CLI:

```bash
cargo run -p arvalez-cli -- generate --openapi openapi.json --output-root generated --no-typescript
```

Disable a backend from config:

```toml
[generator.typescript]
disabled = true
package_name = "@arvalez/client"
```

The Go backend also supports bundled default Tera templates with selective overrides:

```bash
cargo run -p arvalez-cli -- generate-go --openapi openapi.json --template-dir ./templates/go
```

Supported override names are:

- `package/go.mod.tera`
- `package/README.md.tera`
- `package/models.go.tera`
- `package/client.go.tera`
- `partials/model_struct.go.tera`
- `partials/service.go.tera`
- `partials/client_method.go.tera`

When `group_by_tag = true`, tagged operations are grouped under subclients. For example, Python becomes `client.ingredients.create_ingredient(...)` and TypeScript becomes `client.ingredients.createIngredient(...)`. Operations without tags stay on the root client, and multi-tag operations use the first tag.

Shared settings in `[generator]` act as defaults, and `[generator.python]` / `[generator.typescript]` can override them per target. That includes `group_by_tag`, `version`, and similar cross-target options.

Override only selected Python templates:

```bash
cargo run -p arvalez-cli -- generate-python --openapi openapi.json --template-dir ./templates/python
```

The Python backend ships with bundled default Tera templates inside the binary. Override files are optional and can be provided selectively. Supported override names are:

- `package/pyproject.toml.tera`
- `package/README.md.tera`
- `package/__init__.py.tera`
- `package/models.py.tera`
- `package/client.py.tera`
- `partials/model_class.py.tera`
- `partials/client_class.py.tera`
- `partials/client_method.py.tera`

Override only selected TypeScript templates:

```bash
cargo run -p arvalez-cli -- generate-typescript --openapi openapi.json --template-dir ./templates/typescript
```

The TypeScript backend also ships with bundled default Tera templates inside the binary. Supported override names are:

- `package/package.json.tera`
- `package/tsconfig.json.tera`
- `package/README.md.tera`
- `package/models.ts.tera`
- `package/client.ts.tera`
- `package/index.ts.tera`
- `partials/model_interface.ts.tera`
- `partials/client_method.ts.tera`
- `partials/tag_group.ts.tera`

Inspect the raw fixture IR:

```bash
cargo run -p arvalez-cli -- inspect-ir
```

## Docker

Build a container image with the precompiled CLI:

```bash
docker build -t arvalez .
```

Run the bundled tool against files mounted from the current workspace:

```bash
docker run --rm -v "$PWD:/work" -w /work arvalez build-ir --openapi openapi.json
```

Generate a Python SDK from inside the container:

```bash
docker run --rm -v "$PWD:/work" -w /work arvalez generate-python --openapi openapi.json --output generated/python-client
```
