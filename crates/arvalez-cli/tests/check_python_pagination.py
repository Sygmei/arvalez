import asyncio
import inspect
import sys
sys.path.insert(0, sys.argv[1])
import httpx
import sdk.client as generated

mini = sys.argv[2] == "python-mini"
grouped = sys.argv[3] == "true" and not mini
names = ["page_zero", "page_one", "total_items", "last_page", "more", "short", "empty", "offset", "cursor", "root", "nested"]

def make_client(name, mode="normal", asynchronous=False):
    calls = []
    def handle(request):
        calls.append(request)
        n = len(calls)
        assert n < 8, "did not stop"
        q = request.url.params
        assert q["filter"] == "active & ready" and request.headers["auth"] == "token"
        assert request.url.path.startswith("/team/")
        assert q["limit"] == "2"
        assert q["models"] == "model-filter"
        if name == "cursor":
            assert ("cursor" not in q) if n == 1 else q["cursor"] == "token/+=?"
        elif name == "offset":
            assert q["offset"] == str(n + 1)
        else:
            assert q["page"] == str(n - 1 if name in ["page_zero", "last_page"] else n)
        if mode == "http" and n == 2:
            return httpx.Response(500, json={"error": "failed"})
        if mode == "bad_json":
            return httpx.Response(200, text="{")
        items = [{"id": str(n)}]
        if name in ["short", "empty", "root"] and n == 1:
            items = [{"id":"1"}, {"id":"2"}]
        if name in ["empty", "root"] and n == 2:
            items = []
        body = dict(items=items, pages=2, total=4 if name == "offset" else 2, last=1, more=n < 2)
        if name == "cursor" and (n == 1 or mode == "loop"):
            body["next"] = "token/+=?"
        if mode == "null": body["items"] = None
        if mode == "metadata": body["pages"] = True
        if mode == "empty_more": body["items"] = []
        if mode == "bad_cursor": body["next"] = 5
        if name == "root": body = items
        if name == "nested": body = {"a/b": body}
        return httpx.Response(200, json=body)
    transport = httpx.MockTransport(handle)
    if mini:
        client = generated.ApiClient("https://example.test", transport=transport)
    elif asynchronous:
        client = generated.AsyncApiClient(httpx.AsyncClient(base_url="https://example.test", transport=transport))
    else:
        client = generated.SyncApiClient(httpx.Client(base_url="https://example.test", transport=transport))
    return client, client.widgets if grouped else client, calls

args = dict(tenant="team", filter="active & ready", auth="token", models="model-filter")
for name in names:
    client, api, calls = make_client(name)
    items = getattr(api, name+"_collect")(**args)
    assert len(items) == (3 if name == "short" else 2)
    assert all(isinstance(item, generated.models.Widget) for item in items)
    assert len(calls) == 2
    client.close()

for mode in ["http", "bad_json", "null", "metadata", "empty_more", "loop", "bad_cursor"]:
    name = "cursor" if mode in ["loop", "bad_cursor"] else "page_zero"
    client, api, calls = make_client(name, mode)
    try:
        getattr(api, name+"_collect")(**args)
        raise AssertionError(f"accepted {mode}")
    except (ValueError, httpx.HTTPStatusError): pass
    client.close()

client, api, calls = make_client("page_zero")
pages = api.page_zero_paginator(**args)
assert not calls
assert len(next(pages)) == 1 and len(calls) == 1
pages.close()
for size in [0, -1, 4, True, 1.5]:
    try:
        list(api.page_zero_paginator(**args, limit=size))
        raise AssertionError(f"accepted size {size}")
    except ValueError: pass
assert len(calls) == 1
assert not hasattr(api, "disabled_collect")
client.close()

if not mini:
    client, api, calls = make_client("page_zero")
    options = {"params":{"page":88,"limit":"2"}, "headers":{"X-Trace":"yes"}}
    api.page_zero_collect(**args, request_options=options)
    assert options["params"]["page"] == 88
    assert calls[0].headers["X-Trace"] == "yes"
    client.close()

    async def check_async():
        for name in names:
            client, api, calls = make_client(name, asynchronous=True)
            pages = getattr(api, name+"_paginator")(**args)
            assert not calls
            items = [item async for page in pages for item in page]
            assert len(items) == (3 if name == "short" else 2) and len(calls)==2
            await client._client.aclose()
        client, api, calls = make_client("cursor", "loop", asynchronous=True)
        try:
            await api.cursor_collect(**args)
            raise AssertionError("async cursor loop")
        except ValueError: pass
        await client._client.aclose()
        client, api, calls = make_client("page_zero", asynchronous=True)
        assert len(await api.page_zero_collect(**args)) == 2
        await client._client.aclose()
        # Cancellation interrupts an in-flight request, never returning partial results.
        entered = asyncio.Event()
        async def blocked(request):
            entered.set()
            await asyncio.Event().wait()
        transport = httpx.MockTransport(blocked)
        owner = generated.AsyncApiClient(httpx.AsyncClient(base_url="https://example.test", transport=transport))
        api = owner.widgets if grouped else owner
        task = asyncio.create_task(api.page_zero_collect(**args))
        await entered.wait()
        task.cancel()
        try:
            await task
            raise AssertionError("cancel ignored")
        except asyncio.CancelledError: pass
        await owner._client.aclose()
    asyncio.run(check_async())
