# arvalez

Modern OpenAPI client generator with a compiler-style core and typed IR.

## Current Scope

This first implementation slice focuses on the OpenAPI importer and the target backends:

- shared IR types in Rust
- a first Python backend that emits Pydantic models plus sync and async `httpx` clients
- a first TypeScript backend that emits typed models plus a `fetch`-based client
- a first Go backend that emits typed models plus a `net/http` client

The OpenAPI importer is in place, and the generator can emit Python, TypeScript, and Go packages from `openapi.json`.

## Named operation inputs

Arvalez 2 generates TypeScript input objects and Go input structs:

```ts
await client.getWidget({ widgetId: 'abc', page: 0 });
```

```go
params := (&sdk.GetWidgetParams{WidgetID: "abc"}).WithPage(0)
widget, err := client.GetWidget(ctx, params)
```

Required Go fields are plain values; optional setters preserve explicit zero
values without pointer helpers. Nullable model fields additionally support
`WithXNull()`, and `WithoutX()` clears optional fields. Transport settings remain
separate from API inputs. Python calling conventions are unchanged.

This is a breaking change to generated Go and TypeScript SDKs. See the
[migration guide](MIGRATION.md) for call-site changes, omission/null semantics,
custom template changes, and downstream versioning.

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
[common.output]
directory = "generated"
group_by_tag = true
skip_deprecated_operations = false

[common.package]
name = "arvalez-client"
version = "1.0.0"

[target.go]
module_path = "github.com/acme/client"

[target.go.package]
name = "client"
version = "1.0.0"

[target.python]
template_dir = "./templates/python"
# Keep operation parameters positional unless explicitly enabled.
keyword_only = false

[target.python.package]
name = "arvalez_client"
version = "1.0.1"

[target.python.output]
skip_deprecated_operations = true

[target.typescript]
template_dir = "./templates/typescript"

[target.typescript.package]
name = "@arvalez/client"
version = "1.0.2"
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
[target.typescript]
disabled = true

[target.typescript.package]
name = "@arvalez/client"
```

The Go backend also supports bundled default Tera templates with selective overrides:

```bash
cargo run -p arvalez-cli -- generate-go --openapi openapi.json --template-dir ./templates/go
```

Supported override names are:

- `root/go.mod.tera`
- `root/README.md.tera`
- `root/models.go.tera`
- `root/params.go.tera`
- `root/client.go.tera`
- `root/utils.go.tera`
- `partials/input_setters.go.tera`
- `partials/model_struct.go.tera`
- `partials/service.go.tera`
- `partials/client_method.go.tera`
- `partials/array_parameter.go.tera`

When `group_by_tag = true`, tagged operations are grouped under subclients. For example, Python becomes `client.ingredients.create_ingredient(...)` and TypeScript becomes `client.ingredients.createIngredient(...)`. Operations without tags stay on the root client, and multi-tag operations use the first tag.

Shared settings in `[common.output]` and `[common.package]` act as defaults, and `[target.python.output]` / `[target.typescript.output]` / `[target.go.output]` can override output settings per target. That includes `group_by_tag`, `skip_deprecated_operations`, output directories, and package metadata such as `version`. Python operation parameters are positional by default; set `[target.python].keyword_only = true` or pass `--keyword-only` to `generate-python` to require keyword arguments. CLI flags like `--output-version` override config values.

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

To **suppress** a default output file entirely, place an empty file whose name starts with `~` in the same directory as the template it targets. For example, placing `package/~pyproject.toml.tera` in your template directory will prevent `pyproject.toml` from being generated at all.

Override only selected TypeScript templates:

```bash
cargo run -p arvalez-cli -- generate-typescript --openapi openapi.json --template-dir ./templates/typescript
```

The TypeScript backend also ships with bundled default Tera templates inside the binary. Supported override names are:

- `root/package.json.tera`
- `root/tsconfig.json.tera`
- `root/README.md.tera`
- `root/src/models.ts.tera`
- `root/src/params.ts.tera`
- `root/src/client.ts.tera`
- `root/src/utils.ts.tera`
- `root/src/index.ts.tera`
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

`Dockerfile.apiclient` validates `OPENAPI_DOCUMENT` with Redocly CLI's `spec`
ruleset before any SDK generation stage runs.

## Releases

Publishing a GitHub release triggers `.github/workflows/release.yml`, which:

- checks that the release tag matches the workspace version in `Cargo.toml`
- uploads prebuilt `arvalez-cli` binaries for Windows, macOS, and Linux on x86 and arm64 targets
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

## Terraform adapters for Go SDKs

The opt-in Terraform target generates a separate Go module importing an existing
Arvalez Go SDK. Microservice authors maintain their normal OpenAPI contract and
SDK; provider builders own the Terraform configuration and adapter module. The
SDK has no Terraform dependency and requires no Terraform-specific annotations.

```toml
[target.terraform]
module_path = "github.com/acme/orchestrator-tfmodels"
sdk_module_path = "github.com/acme/orchestrator-client/v2"
sdk_version = "v2.0.0"
# sdk_import_path defaults to sdk_module_path; set it for an SDK subpackage.

[target.terraform.package]
name = "tfmodels"

# Original IR model/field names identify these optional overrides.
[target.terraform.models.Widget.fields.payload]
representation = "json"

[target.terraform.models.Widget.fields.displayName]
name = "display_name"
null_behavior = "explicit_null" # requires an optional nullable SDK field
```

```sh
cargo run -p arvalez-cli -- generate-terraform --openapi openapi.json
cargo run -p arvalez-cli -- dump-templates --target terraform
```

Output defaults to `generated/terraform`. The aggregate `generate` command only
includes this target when `[target.terraform]` exists and is enabled;
`--no-terraform` disables it. The dedicated command also accepts
`--module-path`, `--sdk-module-path`, `--sdk-version`, `--sdk-import-path`,
`--package-name`, `--output-directory`, and `--template-dir` overrides.
Module paths and the pinned SDK version are required. Generate adapters from the
same specification and Arvalez version as the standard Go SDK.

For each object `Widget`, the module provides `WidgetModel`, `WidgetFromAPI`,
`WidgetToAPI`, `WidgetAttributeTypes`, `WidgetResourceAttributes`, and
`WidgetDataSourceAttributes`. Named enums have scalar counterparts and
conversions. Schema helpers default every field to Computed; provider authors
configure input roles and nested field options in Go through `SchemaOptions`.
They can add their own validators and plan modifiers to the returned schemas.

Adapters preserve zero values and handle nullable SDK wrappers explicitly.
Unknown outbound values produce path-specific diagnostics. Optional API fields
are omitted on Terraform null unless an optional nullable field opts into
explicit JSON null. API omission and API null both become Terraform null, so
conversion does not promise lossless preservation of that distinction.

Unions, free-form JSON, tuples, and recursive edges need explicit JSON-string
fallback configuration. Unsupported structures, invalid overrides, and naming
collisions are rejected before Terraform files are written. Arrays map to lists;
binary data uses base64 strings. Generation does not install dependencies.

Generated modules require Go 1.24+ and pin Plugin Framework v1.18.0. Run
`go mod tidy` after generation. Use a parent `go.work` to develop alongside an
unpublished SDK without putting local replacements in the generated module.
The generated README contains schema, state, and request conversion examples.
Provider authors retain ownership of CRUD, identity/imports, replacement rules,
and reconciliation when API responses normalize configured values.

Nullability follows the shared IR and the SDK representation. Both OpenAPI 3.0
`type: string` with `nullable: true` and OpenAPI 3.1 `type: ["string", "null"]`
produce nullable SDK fields and Terraform conversions. In OpenAPI 3.0, declare
`nullable` alongside an explicit `type`; for a `$ref`, put it on the referenced
schema. Required nullable fields use `Nullable[T]`; optional nullable fields use
`*Nullable[T]` to distinguish omission from explicit null. Regenerate both the
SDK and adapters when adopting nullable declarations.
