# Arvalez

Generate typed API clients from OpenAPI, with shared configuration and customizable templates.

| Target | Generated code |
| --- | --- |
| [Go](https://github.com/Sygmei/arvalez/wiki/Go) | Typed models and a `net/http` client |
| [TypeScript](https://github.com/Sygmei/arvalez/wiki/TypeScript) | Typed models and a `fetch` client |
| [Python](https://github.com/Sygmei/arvalez/wiki/Python) | Pydantic models and sync/async `httpx` clients |
| [PythonMini](https://github.com/Sygmei/arvalez/wiki/PythonMini) | A compact Python client and Pydantic models |
| [Nushell](https://github.com/Sygmei/arvalez/wiki/Nushell) | Client commands and model constructors |
| [Terraform](https://github.com/Sygmei/arvalez/wiki/Terraform-adapters) | Opt-in adapters for Arvalez Go SDKs |

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

All client targets generate lazy paginators and collection helpers using service-wide pagination settings and per-operation overrides. See [automatic pagination](PAGINATION.md) for page, offset, and cursor configuration.

## Documentation

The **[repository wiki](https://github.com/Sygmei/arvalez/wiki)** has two main sections:

- **[Generate libraries with Arvalez](https://github.com/Sygmei/arvalez/wiki/Generating-libraries)** — installation, target configuration, [pagination settings](https://github.com/Sygmei/arvalez/wiki/Pagination-configuration), model policies, and templates.
- **[Use generated libraries](https://github.com/Sygmei/arvalez/wiki/Using-generated-libraries)** — per-language installation, authentication, requests, [pagination helpers](https://github.com/Sygmei/arvalez/wiki/Pagination), errors, and testing.

Upgrading generated Go or TypeScript clients from Arvalez 1? Follow the [Arvalez 2 migration guide](MIGRATION.md).
