import { ApiClient, type GetWidgetParams2, type ListWidgetsParams, type RequestOptions } from './index';

function check(value: unknown, message: string): asserts value {
  if (!value) throw new Error(message);
}

// Type checks are compiled, but deliberately invalid calls are never executed.
function typeChecks(client: ApiClient): void {
  // @ts-expect-error required path input must be supplied
  void client.getWidget({});
  // @ts-expect-error old positional calls are no longer accepted
  void client.getWidget('id', false);
  // @ts-expect-error required body must be supplied
  void client.requiredBody({});
  // @ts-expect-error input field types remain strict
  const wrong: ListWidgetsParams = { page: 'zero' };
  void wrong;
}
void typeChecks;

async function run(): Promise<void> {
  const requests: {url: URL; init: RequestInit}[] = [];
  let status = 200;
  const client = new ApiClient({baseUrl: 'https://example.test', headers: {'X-Default': 'yes'}, fetch: async (input, init) => {
    requests.push({url: new URL(String(input)), init: init ?? {}});
    return new Response('{"id":"result"}', {status, headers: {'Content-Type': 'application/json'}});
  }});
  const last = (): {url: URL; init: RequestInit} => requests[requests.length - 1]!;
  const params: GetWidgetParams2 = {widgetId: 'id', includeCount: false, page: 0, filter: '', XLabel: '', body: {id: 'sent', count: 0, picture_url: null}};
  Object.freeze(params);
  const options: RequestOptions = {headers: {'X-Option': 'yes'}, cookies: {session: 'abc'}};
  const result = await client.getWidget(params, options);
  check(result.id === 'result', 'decoded response');
  check(last().url.pathname === '/widgets/id', 'path');
  check(last().url.searchParams.get('include_count') === 'false' && last().url.searchParams.get('page') === '0' && last().url.searchParams.has('filter'), 'zero values');
  const headers = new Headers(last().init.headers);
  check(headers.has('X-Label') && headers.get('X-Default') === 'yes' && headers.get('X-Option') === 'yes' && headers.get('Cookie') === 'session=abc', 'headers and cookies');
  const body = JSON.parse(String(last().init.body)) as Record<string, unknown>;
  check(body.picture_url === null && body.count === 0, 'body');
  await client._getWidgetRaw({widgetId: 'id'});
  check(last().url.search === '' && last().init.body === undefined, 'omission');
  await client.listWidgets();
  await client.ping();
  await client.ping({}, {headers: {'X-Only': 'yes'}});
  check(new Headers(last().init.headers).get('X-Only') === 'yes', 'options-only request');
  await client.listWidgets({page: 0, enabled: false, filter: ''}, {query: {page: 9}});
  check(last().url.searchParams.get('page') === '9' && last().url.searchParams.get('enabled') === 'false' && last().url.searchParams.has('filter'), 'query override');
  await client.requiredBody({body: {id: 'required'}});
  check(String(last().init.body).includes('required'), 'required body');
  await client.untypedBody({body: null});
  check(last().init.body === 'null', 'explicit null body');
  await client.stream({body: new Uint8Array([1, 2])});
  check(last().init.body instanceof Uint8Array, 'stream body');
  await client.multipart({body: {id: 'form'}});
  const form = last().init.body;
  check(form instanceof FormData && form.get('id') === 'form', 'multipart');
  await client.collision({body: 'path', params: 'p', requestOptions: 'r', aB: 'one', aB_: 'two', body_: {id: 'body'}});
  check(last().url.pathname === '/collision/path' && last().url.searchParams.get('a-b') === 'one' && last().url.searchParams.get('a_b') === 'two', 'collisions');
  status = 400;
  check((await client._getWidgetRaw({widgetId: 'id'})).status === 400, 'raw HTTP errors');
  let handled = 0;
  await client.getWidget({widgetId: 'id'}, {onError: () => { handled++; }});
  check(handled === 1, 'error override');
}
void run();
