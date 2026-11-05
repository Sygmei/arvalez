def "http get" [url: string --headers: record] {
    let n = ((open --raw $env.PAGINATION_TRACE | lines | length) + 1)
    if $n > 7 { error make {msg: 'did not stop'} }
    $"($url)\n" | save --append $env.PAGINATION_TRACE
    let parsed = ($url | url parse)
    let name = ($parsed.path | split row '/' | last)
    let q = ($parsed.params | transpose --as-record --header-row)
    if $q.pagination_state != 'retained' or $q.limit != '2' or $q.filter != 'active & ready' or $headers.auth != 'token' { error make {msg: 'lost input or wrong size'} }
    if $name == 'Cursor' {
        if $n == 1 { if ('cursor' in ($q | columns)) { error make {msg: 'first cursor was sent'} } } else {
            if $q.cursor != 'token/+=?' { error make {msg: 'cursor encoding'} }
        }
    } else if $name == 'Offset' {
        if $q.offset != (($n + 1) | into string) { error make {msg: 'wrong offset'} }
    } else {
        let expected = if $name in ['PageZero' 'LastPage'] { $n - 1 } else { $n }
        if $q.page != ($expected | into string) { error make {msg: 'wrong page'} }
    }
    if $env.PAGINATION_MODE == 'http' and $n == 2 { error make {msg: 'HTTP 500'} }
    mut items = [{id: ($n | into string)}]
    if $name in ['Short' 'Empty' 'Root'] and $n == 1 { $items = [{id: '1'} {id: '2'}] }
    if $name in ['Empty' 'Root'] and $n == 2 { $items = [] }
    mut payload = {items: $items, pages: 2, total: (if $name == 'Offset' { 4 } else { 2 }), last: 1, more: ($n < 2)}
    if $name == 'Cursor' and ($n == 1 or $env.PAGINATION_MODE == 'loop') { $payload = ($payload | insert next 'token/+=?') }
    if $env.PAGINATION_MODE == 'null' { $payload.items = null }
    if $env.PAGINATION_MODE == 'metadata' { $payload.pages = true }
    if $env.PAGINATION_MODE == 'empty_more' { $payload.items = [] }
    if $name == 'Root' { $items } else if $name == 'Nested' { {'a/b': $payload} } else { $payload }
}
source client.nu
$env.PAGINATION_TRACE = (pwd | path join 'requests.log')
$env.PAGINATION_MODE = 'normal'
let cases = [
    {name: 'zero', run: {|| @GROUP@page-zero-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
    {name: 'one', run: {|| @GROUP@page-one-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
    {name: 'total', run: {|| @GROUP@total-items-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
    {name: 'last', run: {|| @GROUP@last-page-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
    {name: 'more', run: {|| @GROUP@more-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
    {name: 'short', run: {|| @GROUP@short-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
    {name: 'empty', run: {|| @GROUP@empty-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
    {name: 'offset', run: {|| @GROUP@offset-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
    {name: 'cursor', run: {|| @GROUP@cursor-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
    {name: 'root', run: {|| @GROUP@root-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
    {name: 'nested', run: {|| @GROUP@nested-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained}}
]
for case in $cases {
    '' | save --force $env.PAGINATION_TRACE
    let items = (do $case.run)
    let expected = if $case.name == 'short' { 3 } else { 2 }
    if ($items | length) != $expected or (open --raw $env.PAGINATION_TRACE | lines | length) != 2 { error make {msg: $"wrong result for ($case.name)"} }
}
for mode in ['http' 'null' 'metadata' 'empty_more' 'loop'] {
    $env.PAGINATION_MODE = $mode
    '' | save --force $env.PAGINATION_TRACE
    let failed = try {
        if $mode == 'loop' { @GROUP@cursor-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained | ignore } else {
            @GROUP@page-zero-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained | ignore
        }
        false
    } catch { true }
    if not $failed { error make {msg: $"accepted error ($mode)"} }
}
$env.PAGINATION_MODE = 'normal'
for size in [0 -1 4] {
    '' | save --force $env.PAGINATION_TRACE
    let failed = try { @GROUP@page-zero-collect team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained --limit $size | ignore; false } catch { true }
    if not $failed or (open --raw $env.PAGINATION_TRACE | is-not-empty) { error make {msg: 'invalid size sent request'} }
}
'' | save --force $env.PAGINATION_TRACE
let first = (@GROUP@page-zero-paginator team --base-url https://example.test --filter 'active & ready' --auth token --pagination-state retained | first)
if ($first.items | length) != 1 or (open --raw $env.PAGINATION_TRACE | lines | length) != 1 { error make {msg: 'pagination was not lazy'} }
