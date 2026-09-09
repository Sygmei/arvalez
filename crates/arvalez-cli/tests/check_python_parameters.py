"""Exercise generated clients with HTTPX's request encoder and mock transport."""
import asyncio
import sys

sys.path[:0] = [sys.argv[1] + '/python/src', sys.argv[1] + '/python-mini/src']
import httpx
from arvalez_client.client import SyncApiClient, AsyncApiClient
from client.client import ApiClient

requests = []
def handle(request):
    requests.append(request)
    return httpx.Response(200, json={})

values = dict(tofu_version=[1, 2], status=['active', 'paused'], namespace=['a', 'b'],
              action=[True, False], wid=['w1', 'w2'], x_flags=[True, False])
def check():
    request = requests[-1]
    for name, expected in {'status': ['active,paused'], 'namespace': ['a', 'b'],
                           'tofu_version': ['1,2'], 'action': ['true,false'], 'wid': ['w1', 'w2']}.items():
        assert request.url.params.get_list(name) == expected, (name, request.url)
    assert request.headers['X-Flags'] == 'true,false'

def check_empty():
    assert not requests[-1].url.query
    assert 'X-Flags' not in requests[-1].headers

with httpx.Client(base_url='https://example.test', transport=httpx.MockTransport(handle)) as http:
    client = SyncApiClient(http)
    client._list_workspaces_raw(**values)
    check()
    client._list_workspaces_raw(tofu_version=[], status=[], namespace=[], action=[], wid=[], x_flags=[])
    check_empty()

with ApiClient('https://example.test', transport=httpx.MockTransport(handle)) as client:
    client.list_workspaces(**values)
    check()
    client.list_workspaces(tofu_version=[], status=[], namespace=[], action=[], wid=[], x_flags=[])
    check_empty()

async def run():
    async with httpx.AsyncClient(base_url='https://example.test', transport=httpx.MockTransport(handle)) as http:
        client = AsyncApiClient(http)
        await client._list_workspaces_raw(**values)
        check()
asyncio.run(run())
