const assert = require('node:assert/strict');
const {ApiClient} = require(process.argv[2]);
const grouped = process.argv[3] === 'true';
const names = ['pageZero','pageOne','totalItems','lastPage','more','short','empty','offset','cursor','root','nested'];
const args = {tenant:'team', filter:'active & ready', auth:'token'};
function clientFor(name, mode='normal') {
  const calls=[];
  const client = new ApiClient({baseUrl:'https://example.test', fetch:async (url, init) => {
    const q=new URL(url).searchParams;
    calls.push({q,init}); const n=calls.length;
    assert.ok(n<8);
    assert.equal(q.get('filter'),args.filter);
    assert.equal(new Headers(init.headers).get('auth'),'token');
    assert.equal(q.get('limit'),'2');
    if(name==='cursor') assert.equal(q.get('cursor'),n===1?null:'token/+=?');
    else if(name==='offset') assert.equal(q.get('offset'),String(n+1));
    else assert.equal(q.get('page'),String(['pageZero','lastPage'].includes(name)?n-1:n));
    if(mode==='http' && n===2) return new Response('{}',{status:500});
    if(mode==='bad_json') return new Response('{');
    let items=[{id:String(n)}];
    if(['short','empty','root'].includes(name) && n===1) items=[{id:'1'},{id:'2'}];
    if(['empty','root'].includes(name) && n===2) items=[];
    let body={items,pages:2,total:name==='offset'?4:2,last:1,more:n<2};
    if(name==='cursor' && (n===1 || mode==='loop')) body.next='token/+=?';
    if(mode==='null') body.items=null;
    if(mode==='metadata') body.pages=true;
    if(mode==='empty_more') body.items=[];
    if(mode==='bad_cursor') body.next=5;
    if(name==='root') body=items;
    if(name==='nested') body={'a/b':body};
    return new Response(JSON.stringify(body));
  }});
  return {api:grouped?client.widgets:client,calls};
}
(async()=>{
  for(const name of names) {
    const {api,calls}=clientFor(name);
    const items=await api[name[0].toUpperCase()+name.slice(1)+'Collect'](args);
    assert.equal(items.length,name==='short'?3:2);
    assert.equal(calls.length,2);
  }
  for(const mode of ['http','bad_json','null','metadata','empty_more','loop','bad_cursor']) {
    const name=['loop','bad_cursor'].includes(mode)?'cursor':'pageZero';
    const {api}=clientFor(name,mode);
    await assert.rejects(api[name[0].toUpperCase()+name.slice(1)+'Collect'](args),undefined,mode);
  }
  const {api,calls}=clientFor('pageZero');
  const pages=api.PageZeroPaginator(args);
  assert.equal(calls.length,0);
  assert.equal((await pages.next()).value.length,1);
  assert.equal(calls.length,1);
  await pages.return();
  for(const limit of [0,-1,4,true,1.5,NaN,Infinity]) await assert.rejects(api.PageZeroCollect({...args,limit}));
  assert.equal(calls.length,1);
  assert.equal(api.DisabledCollect,undefined);
  const next=clientFor('pageZero');
  const query=new URLSearchParams({page:'88',limit:'2'});
  await next.api.PageZeroCollect(args,{query,headers:{'X-Trace':'yes'}});
  assert.equal(query.get('page'),'88');
  assert.equal(new Headers(next.calls[0].init.headers).get('X-Trace'),'yes');
  const cancellation=clientFor('pageZero');
  const controller=new AbortController();
  const iterator=cancellation.api.PageZeroPaginator(args,{signal:controller.signal});
  await iterator.next(); controller.abort();
  await assert.rejects(iterator.next());
  assert.equal(cancellation.calls.length,1);
})().catch(error=>{console.error(error);process.exitCode=1});
