# arvalez

Modern OpenAPI client generator with a compiler-style core and typed IR.

## Current Scope

This first implementation slice focuses on the OpenAPI importer and the target backends:

- shared IR types in Rust
- a first Python backend that emits Pydantic models plus sync and async `httpx` clients
- a first TypeScript backend that emits typed models plus a `fetch`-based client
- a first Go backend that emits typed models plus a `net/http` client

The OpenAPI importer is in place, and the generator can emit Python, TypeScript, and Go packages from `openapi.json`.

## Workspace Layout

- `crates/arvalez-ir`: shared IR and validation
- `crates/arvalez-openapi`: OpenAPI document loader and Core IR mapper
- `crates/arvalez-target-go`: Go SDK generator
- `crates/arvalez-target-python`: Python SDK generator
- `crates/arvalez-target-typescript`: TypeScript SDK generator
- `crates/arvalez-cli`: local CLI for inspecting IR and generating SDKs

## Build

Build Core IR from the local OpenAPI document:

```bash
cargo run -p arvalez-cli -- build-ir --openapi openapi.json
```

Add `--timings` to print a per-phase breakdown for import and generation work:

```bash
cargo run -p arvalez-cli -- generate --openapi openapi.json --output-directory generated --timings
```

Generate a Python SDK package:

```bash
cargo run -p arvalez-cli -- generate-python --openapi openapi.json --output-directory generated/python-client
```

Generate a TypeScript SDK package:

```bash
cargo run -p arvalez-cli -- generate-typescript --openapi openapi.json --output-directory generated/typescript-client
```

Generate a Go SDK package:

```bash
cargo run -p arvalez-cli -- generate-go --openapi openapi.json --output-directory generated/go-client
```

The generators also read optional settings from `arvalez.toml`:

```toml
[output]
directory = "generated"
group_by_tag = true
version = "1.0.0"

[output.go]
module_path = "github.com/acme/client"
package_name = "client"
version = "1.0.0"

[output.python]
package_name = "arvalez_client"
version = "1.0.1"
template_dir = "./templates/python"

[output.typescript]
package_name = "@arvalez/client"
version = "1.0.2"
template_dir = "./templates/typescript"
```

Override the configured output version from the CLI:

```bash
cargo run -p arvalez-cli -- generate-python --openapi openapi.json --output-directory generated/python-client --output-version 2.3.4
```

Generate all enabled backends into one output root:

```bash
cargo run -p arvalez-cli -- generate --openapi openapi.json --output-directory generated
```

Run the APIs.guru corpus as an on-demand generation test:

```bash
cargo run -p arvalez-cli -- test-apis-guru --report-directory reports/apis-guru
```

By default this command clones `APIs-guru/openapi-directory`, discovers every `openapi.json`, `openapi.yaml`, `swagger.json`, and `swagger.yaml`, and runs generation for each spec. Generated outputs go to a temporary directory unless you pass `--output-directory`, so the repository stays slim.
The checkout is cached under `.arvalez/corpus/openapi-directory/` in the current workspace and refreshed on later runs, so you do not pay the full clone cost every time.
Reports are written into the chosen directory only as timestamped JSON files like `apis-guru-1774593522.json`.
The report dashboard is now a SvelteKit app in [web/corpus-dashboard](/Users/sygmei/Projects/arvalez/web/corpus-dashboard). It serves the report directory through a small local server, watches for new report JSON files, and updates the UI in real time.
For local dashboard development, run `npm install` once in `web/corpus-dashboard/`, then:

```bash
REPORT_DIRECTORY=/absolute/path/to/reports/apis-guru npm run dev
```

If `REPORT_DIRECTORY` is not set, the app defaults to `../../reports/apis-guru` relative to `web/corpus-dashboard/`.
Use `--jobs N` to control spec-level parallelism; by default Arvalez uses the machine's available parallelism.
Pass `--ui` on a local terminal to get a live `ratatui` dashboard with progress, active specs, and recent completions. Press `q` to hide the UI and fall back to plain progress lines while the run continues.

Disable a backend from the CLI:

```bash
cargo run -p arvalez-cli -- generate --openapi openapi.json --output-directory generated --no-typescript
```

Disable a backend from config:

```toml
[output.typescript]
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

Shared settings in `[output]` act as defaults, and `[output.python]` / `[output.typescript]` / `[output.go]` can override them per target. That includes `group_by_tag`, `version`, and similar cross-target options. CLI flags like `--output-version` override both.

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
docker run --rm -v "$PWD:/work" -w /work arvalez generate-python --openapi openapi.json --output-directory generated/python-client
```

## Releases

Publishing a GitHub release triggers `.github/workflows/release.yml`, which:

- checks that the release tag matches the workspace version in `Cargo.toml`
- builds and pushes the multi-arch `arvalez/cli` image to Docker Hub
- publishes the Rust crates to crates.io in dependency order

The workflow expects these GitHub repository secrets:

- `DOCKERHUB_USERNAME`
- `DOCKERHUB_TOKEN`
- `CARGO_REGISTRY_TOKEN`

Set up `pre-commit` once after cloning:

```bash
pre-commit install
```

The `pre-commit` config keeps internal workspace dependency versions aligned with the workspace version and regenerates `Cargo.lock` whenever Cargo manifests are part of the commit.
