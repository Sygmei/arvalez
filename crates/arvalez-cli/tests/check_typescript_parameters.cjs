const assert = require('node:assert/strict');
const { ApiClient } = require(process.argv[2]);
const requests = [];
const client = new ApiClient({baseUrl: 'https://example.test', fetch: async (url, init) => {
  requests.push({url: new URL(url), headers: new Headers(init.headers)});
  return new Response('{}');
}});
(async () => {
  await client._listWorkspacesRaw([1, 2], ['active', 'paused'], ['a', 'b'], [true, false], ['w1', 'w2'], [true, false]);
  const request = requests.at(-1);
  for (const [name, expected] of Object.entries({status: ['active,paused'], namespace: ['a', 'b'],
    tofu_version: ['1,2'], action: ['true,false'], wid: ['w1', 'w2']})) {
    assert.deepEqual(request.url.searchParams.getAll(name), expected);
  }
  assert.equal(request.headers.get('X-Flags'), 'true,false');
  await client._listWorkspacesRaw([], [], [], [], [], []);
  assert.equal(requests.at(-1).url.search, '');
  assert.equal(requests.at(-1).headers.has('X-Flags'), false);
})().catch(error => { console.error(error); process.exitCode = 1; });
