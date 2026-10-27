# Migrating generated SDKs to Arvalez 2

Arvalez 2 generates named input objects for TypeScript and input structs for Go.
This replaces positional operation arguments for both decoded and raw methods,
including tag-grouped clients. There is no legacy signature mode. Python and
Python Mini calling conventions are unchanged, including Python's `keyword_only`
setting. Nushell generation is unchanged.

## TypeScript

Move path, query, header, cookie, and body inputs into the operation's exported
`<Operation>Params` interface. Transport settings remain the second argument.
For example, given a required `widget_id` path parameter and optional `page` query:

```ts
// Before
await client.getWidget('abc', 0, undefined, { headers: { 'X-Trace': '123' } });

// After
await client.getWidget(
  { widgetId: 'abc', page: 0 },
  { headers: { 'X-Trace': '123' } },
);
```

The before example includes the old unused body argument that could be emitted
for operations without a request body. It is no longer generated.

Request bodies use `body` inside the input object. Required fields remain required
by TypeScript; optional fields can be omitted. Explicit `false`, `0`, and `''`
are preserved. JSON `null` and omitted properties retain their usual serialization
semantics. Input objects are not mutated by the generated methods.

When every input is optional, the input object defaults to `{}`:

```ts
await client.listWidgets();
await client.listWidgets({ page: 0 });
await client.listWidgets({}, { headers: { 'X-Trace': '123' } });
```

Input types live in `src/params.ts` and are re-exported from the package index.
Use the generated field names; names still follow the existing identifier
sanitizer, so a header such as `X-Label` becomes `XLabel`.

## Go

Pass a pointer to an operation's `<Operation>Params` struct after `context.Context`.
Required inputs are ordinary fields. Optional scalar inputs use pointers;
`WithX(value)` setters create those pointers and return the same receiver.
`WithoutX()` restores the omitted state.

```go
// Before
widget, err := client.GetWidget(ctx, "abc", &page, requestOptions)

// After
params := (&sdk.GetWidgetParams{WidgetID: "abc"}).WithPage(0)
widget, err := client.GetWidget(ctx, params,
    sdk.WithRequestOptions(sdk.RequestOptions{
        Headers: http.Header{"X-Trace": {"123"}},
    }),
)
```

The package qualifier `sdk` is illustrative. Import your generated module with
that alias, or use its configured package name.

No trailing options argument is needed. Pass `nil` for params when the operation
has no required inputs. Passing `nil` when inputs are required returns an error
before sending a request. Go cannot distinguish an omitted ordinary struct field
from an explicitly supplied zero value: `&GetWidgetParams{}` supplies an empty
`WidgetID`. Required-field validation beyond the nil check remains the caller's
responsibility.

Setters also cover optional fields on generated models. Nullable model fields
have a third operation, `WithXNull()`, for an explicit JSON null:

```go
patch := (&sdk.Widget{ID: "abc"}).WithCount(0).WithPictureURLNull()
params := (&sdk.UpdateWidgetParams{WidgetID: "abc"}).WithBody(*patch)
_, err := client.UpdateWidget(ctx, params)
patch.WithoutPictureURL() // omit on the next serialization
```

These setters use the existing model representation and JSON tags. For example,
non-nullable optional slices and maps still use `omitempty`; empty collections
on those model fields can be omitted. A pointer to an optional whole request
body distinguishes no body from an explicitly supplied nil map, slice, or
interface (JSON null). Streaming bodies remain `io.Reader`.

Setters mutate their receiver; create separate input values for concurrent
mutation. `WithX` takes the underlying value, so no `sdk.Ptr` call is needed.
Required fields are supplied in the struct literal.

Transport options use `...RequestOption`. They run once per call in order,
including decoded calls. `WithRequestOptions` replaces all prior per-call
settings; when repeated, the last one wins. Client defaults still apply through
the existing merge rules. Raw methods continue returning HTTP errors as raw
responses; decoded methods continue using the configured error handler.

## Names and template overrides

Input type names are allocated around existing models, inline enum names, and
runtime/service names. A collision can produce `GetWidgetParams2`. Go field and
setter collisions similarly receive numeric suffixes. TypeScript retains its
existing underscore suffixes for input field collisions. Adding conflicting
schema names can therefore rename generated symbols; review the generated diff.

Custom templates should be updated together with the generator:

- Go adds `root/params.go.tera` and `partials/input_setters.go.tera`.
- TypeScript adds `root/src/params.ts.tera` and exports it from the index.
- Compute `input_types` using `ir | go_input_types` or `ir | ts_input_types` once
  per root template, then pass `types=input_types` to `go_operation` or
  `ts_operation`.
- Operation views expose `params_type`, `input_fields`, and `has_required_inputs`.
  Go parameter identifiers now refer to struct fields rather than local arguments.
- Keep customized client, model, index, and service templates in sync with the
  bundled templates. Suppressing the new params file requires an equivalent
  custom definition of its types.

## Versioning downstream SDKs

Arvalez's workspace version is 2.0.0. Generated SDK package versions remain
controlled by your config or `--output-version`; they are not automatically
increased. Release regenerated SDKs with the appropriate breaking version and
update all consumers. For a Go module moving from v1 to v2, update its configured
module path and consumer imports to the `/v2` path as well as its release tag.
