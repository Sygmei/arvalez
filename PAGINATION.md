# Automatic pagination

Arvalez can generate lazy paginators and collection helpers for GET collection
operations. Configure the microservice's convention once under
`common.pagination`, then override individual operations using their OpenAPI
`operationId` (before language-specific name conversion).

Pagination configuration and schema validation are shared by **Go, TypeScript,
Python (sync and async), PythonMini, and Nushell**. Each client target generates
lazy pagination and collection helpers. Terraform adapters use the generated Go
SDK for requests, including its pagination helpers; the adapters themselves
convert models and do not perform requests.

## Page numbers

```toml
[common.pagination]
strategy = "page"
page_param = "query.page"
page_size_param = "query.page_size"
start_page = 1
default_page_size = 100
max_page_size = 500
items_path = "/items"
total_pages_path = "/pagination/total_pages"

[common.pagination.operations.GetAllEvents]
start_page = 0
default_page_size = 50
max_page_size = 100

[common.pagination.operations.GetAllCountries]
disabled = true
```

- `start_page` is the first page index, defaulting to `1`; `0` is supported.
- `default_page_size` is required and must be positive.
- `max_page_size` is optional. When supplied, it must be positive and at least
  `default_page_size`. Callers requesting more receive an error before any HTTP
  request is sent; sizes are never silently clamped.
- Request selectors use `query.<wire_name>`. Page, offset, and size parameters
  must have integer schemas; cursors must have optional string schemas.
- `items_path` is a JSON Pointer to the result array: `/items`, `/data/widgets`,
  or `""` for a top-level array. Escape `/` in field names as `~1` and `~` as `~0`.
- Response paths use JSON field names, independently of generated model names.

Service defaults apply to compatible GET operations: pagination parameters,
result arrays, and termination metadata must match their schemas. Incompatible
operations are skipped. An explicit operation entry, including an empty table,
requires a compatible schema and fails generation on a mismatch. Unknown
operation names, malformed settings, and conflicting termination rules are
errors. No helpers are generated merely because a method is named `GetAllX`.

An explicit `disabled = true` skips that operation. Set `disabled = false` in an
override to enable it when the service default is disabled. Defaults may also be
omitted entirely, with complete settings supplied for each operation.

## Termination rules

Configure exactly one rule per operation:

| Setting | Meaning | Strategies |
| --- | --- | --- |
| `total_pages_path` | Number of pages, independent of the starting page index | Page |
| `last_page_path` | Inclusive last page index, in the API's numbering system | Page |
| `total_items_path` | Total item count; includes items before `start_offset` | Page, offset |
| `has_more_path` | Boolean indicating whether another page exists | Page, offset |
| `stop_on = "empty_page"` | Stop after receiving an empty array | Page, offset |
| `stop_on = "short_page"` | Stop when fewer items than requested are returned | Page, offset |
| `next_cursor_path` | Next string cursor; missing, null, or empty means finished | Cursor |

For example, with `start_page = 0` and a page count of `3`, the paginator requests
pages `0`, `1`, and `2`. A `last_page_path` value of `3` instead includes page `3`.
Numeric metadata must be nonnegative integers; `has_more` must be a boolean.
Missing or null metadata is an error except for next cursors.

Use `short_page` only when that is the API's documented completion rule. Servers
that cap or variably fill pages can return fewer items even when more exist.
An empty page with metadata saying to continue causes an error for page/offset
pagination, since it indicates no progress. Cursor pagination permits empty
pages when the cursor advances, but detects repeated cursors and cursor cycles.

## Offset and cursor overrides

```toml
[common.pagination.operations.GetAllLogs]
strategy = "offset"
offset_param = "query.offset"
start_offset = 0
total_items_path = "/pagination/total"

[common.pagination.operations.GetAllChanges]
strategy = "cursor"
cursor_param = "query.cursor"
items_path = "/data/changes"
next_cursor_path = "/meta/next_cursor"
```

These examples inherit page-size settings from the service default. Changing
strategy clears inherited page/offset/cursor selectors, starting positions, and
termination rules. Other settings remain inherited. Supplying a new termination
rule replaces the inherited rule even when the strategy remains unchanged.

Offsets default to `0` and advance by the number of returned items. The first
cursor request omits the cursor; subsequent requests send it unchanged, with
normal query encoding.

## Generated Go API

For an existing `GetAllWidgets` operation:

```go
// Fetch all items. On failure, items is nil and err explains the failure.
items, err := client.GetAllWidgetsCollect(ctx, nil)

// Fetch one page at a time and stop early if desired.
size := int64(50)
p := client.GetAllWidgetsPaginator(ctx, &GetAllWidgetsParams{
    PageSize: &size, // optional query parameters are pointers
})
for p.Next() {
    for _, widget := range p.Items() {
        process(widget)
    }
}
if err := p.Err(); err != nil {
    return err
}
```

The concrete params type and fields depend on the operation. Tag-grouped clients
expose the helpers on the same service as the existing method, for example
`client.Widgets.GetAllWidgetsCollect(...)`.

The existing single-page and `Raw` methods remain available. Paginators start at
the configured first page/offset or with no cursor; they manage position inputs
internally, including any position supplied through `RequestOptions.Query`.
Page size is selected from the params, then overridden by a request-option query
value if present, otherwise using the configured default. An unset optional size
uses the default; explicit zero is invalid. For required integer size fields,
zero means use the default, allowing a zero-value params struct.

Filters and request options are retained, and caller-owned pagination inputs
and option maps are not mutated. Request-option functions run once when the
paginator is constructed. No HTTP requests occur until `Next`. Responses are
closed after each page. Paginators use the existing client transport and error
handler and honor context cancellation. They are sequential and not safe for
concurrent use; no new retry behavior is introduced.

`Items()` exposes the current page. After `Next()` returns false, check `Err()`;
errors remain available on subsequent calls. `Collect` keeps all items in memory
and returns nil with the error on failure, never a partial list presented as
complete. Use a context deadline for APIs whose termination signal cannot be
trusted to eventually finish.

Generation rejects helper-name collisions rather than producing invalid Go.
`Paginator` is reserved as the generic generated paginator type when pagination
helpers are emitted. Built-in pagination templates can be overridden using the
usual template mechanism.

## Python and PythonMini

For `GetAllWidgets`, both Python targets generate `get_all_widgets_paginator`
and `get_all_widgets_collect`. Helpers use keyword arguments, preserve required
filters and path parameters, and omit the managed page/offset/cursor argument.
The page-size argument is optional even if the single-page operation requires it.

```python
# SyncApiClient and PythonMini ApiClient:
for page in client.get_all_widgets_paginator(page_size=50):
    for widget in page:
        process(widget)
items = client.get_all_widgets_collect(page_size=50)

# AsyncApiClient (also exported as ApiClient by the full Python target):
async for page in client.get_all_widgets_paginator(page_size=50):
    for widget in page:
        process(widget)
items = await client.get_all_widgets_collect(page_size=50)
```

Pages contain validated, typed items, including Pydantic model instances. The
full Python target retains per-call `request_options`, error handlers, and
HTTPX timeout/cancellation behavior. PythonMini uses its existing HTTPX client
configuration and status-error handling. Closing an iterator stops further
requests; async task cancellation interrupts in-flight requests. A failed page
raises an exception, and collection never returns a partial result.

With tag grouping enabled, the full Python helpers live on the same service as
the single-page method. PythonMini retains its compact flat client layout.

## TypeScript

```typescript
for await (const page of client.GetAllWidgetsPaginator({pageSize: 50}, {signal})) {
  for (const widget of page) process(widget);
}
const items = await client.GetAllWidgetsCollect({pageSize: 50}, {signal});
```

Helper names follow the existing operation name with `Paginator` or `Collect`
appended. Paginators return typed async generators of item arrays. The params
object omits the managed position field and makes page size optional, while
retaining required filters and path parameters. Request options, error handlers,
and abort signals are preserved. Breaking the loop stops additional requests;
collection rejects on any failure. Tag groups expose the helpers alongside
single-page methods. Pagination counters and numeric metadata must be safe
JavaScript integers; unsupported values fail instead of silently losing precision.

## Nushell

```nu
# Stream one record per page, with an `items` list; stop after the first page.
get-all-widgets-paginator --base-url https://api.example.com --page-size 50 | first

# Materialize a flat list of all items; any failed page aborts the command.
get-all-widgets-collect --base-url https://api.example.com --page-size 50
```

Helpers preserve path arguments, filter/header flags, and `--base-url`. The
managed page/offset/cursor flag is omitted. With tag grouping, command names
include the same tag prefix as single-page commands. Lazy streams stop issuing
requests when downstream consumption stops; normal Nushell interruption also
applies. Scalar query values, including opaque cursors, are URL encoded.

## Validation

`cargo test --workspace` includes generation and runtime checks for every client
language. Runtime checks require Go, Node/TypeScript, Nushell, and a Python with
HTTPX and Pydantic 2 installed. Set `ARVALEZ_TEST_PYTHON` to select that interpreter.
Unavailable runtimes are skipped unless `ARVALEZ_REQUIRE_PAGINATION_RUNTIMES=1`
is set; an explicitly selected Python interpreter must have the dependencies.
Tests cover flat and grouped layouts, every strategy and termination rule,
zero/one-based pages, limits, filters/headers, JSON Pointer paths, cancellation,
partial failures, cursor loops, and generated-name collisions.
