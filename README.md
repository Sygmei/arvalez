# Arvalez

Generate typed API clients from OpenAPI, with shared configuration and customizable templates.

| Target | Generated code |
| --- | --- |
| Go | Typed models and a `net/http` client |
| TypeScript | Typed models and a `fetch` client |
| Python | Pydantic models and sync/async `httpx` clients |
| PythonMini | A compact Python client and Pydantic models |
| Nushell | Client commands and model constructors |
| Terraform | Opt-in adapters for Arvalez Go SDKs |

## Quick start

Install with Cargo, or download a [prebuilt binary](https://github.com/Sygmei/arvalez/releases):

```bash
cargo install arvalez-cli --locked
arvalez-cli generate --openapi openapi.json --output-directory generated
```

The aggregate command generates Go, TypeScript, Python, and Nushell clients. Terraform is opt-in; PythonMini uses `generate-python-mini`. To generate one target:

```bash
arvalez-cli generate-go --openapi openapi.json --output-directory generated/go-client
```

Optional settings live in `arvalez.toml`:

```toml
[common.package]
version = "1.0.0"

[target.go]
module_path = "github.com/acme/client"

[target.python.package]
name = "acme_client"
```

For APIs that always include every response field, `response_fields = "always_present"` makes response properties required and simplifies nullable types while preserving request models. See [response model policies](https://github.com/Sygmei/arvalez/wiki/Response-model-policies) before opting in.

## Documentation

The **[repository wiki](https://github.com/Sygmei/arvalez/wiki)** contains the detailed guides:

- [Getting started and Docker](https://github.com/Sygmei/arvalez/wiki/Getting-started)
- [Configuration and overrides](https://github.com/Sygmei/arvalez/wiki/Configuration)
- [Response model policies](https://github.com/Sygmei/arvalez/wiki/Response-model-policies)
- [Template customization](https://github.com/Sygmei/arvalez/wiki/Templates)
- [Terraform adapters](https://github.com/Sygmei/arvalez/wiki/Terraform-adapters)
- [Development, tests, and releases](https://github.com/Sygmei/arvalez/wiki/Development)

Upgrading generated Go or TypeScript clients from Arvalez 1? Follow the [Arvalez 2 migration guide](MIGRATION.md).
