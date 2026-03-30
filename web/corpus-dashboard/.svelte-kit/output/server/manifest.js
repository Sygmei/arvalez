export const manifest = (() => {
function __memo(fn) {
	let value;
	return () => value ??= (value = fn());
}

return {
	appDir: "_app",
	appPath: "_app",
	assets: new Set([]),
	mimeTypes: {},
	_: {
		client: {start:"_app/immutable/entry/start.BZnqYoT8.js",app:"_app/immutable/entry/app.CvcvY1tM.js",imports:["_app/immutable/entry/start.BZnqYoT8.js","_app/immutable/chunks/BtxEEb5p.js","_app/immutable/chunks/4xzEkbiA.js","_app/immutable/chunks/Dp6_12In.js","_app/immutable/entry/app.CvcvY1tM.js","_app/immutable/chunks/4xzEkbiA.js","_app/immutable/chunks/BddMhpxk.js","_app/immutable/chunks/C-zbSqNs.js","_app/immutable/chunks/Dp6_12In.js","_app/immutable/chunks/K-Ov3CMU.js","_app/immutable/chunks/Ccb5uD3K.js"],stylesheets:[],fonts:[],uses_env_dynamic_public:false},
		nodes: [
			__memo(() => import('./nodes/0.js')),
			__memo(() => import('./nodes/1.js')),
			__memo(() => import('./nodes/2.js'))
		],
		remotes: {
			
		},
		routes: [
			{
				id: "/",
				pattern: /^\/$/,
				params: [],
				page: { layouts: [0,], errors: [1,], leaf: 2 },
				endpoint: null
			},
			{
				id: "/api/reports",
				pattern: /^\/api\/reports\/?$/,
				params: [],
				page: null,
				endpoint: __memo(() => import('./entries/endpoints/api/reports/_server.js'))
			},
			{
				id: "/api/reports/stream",
				pattern: /^\/api\/reports\/stream\/?$/,
				params: [],
				page: null,
				endpoint: __memo(() => import('./entries/endpoints/api/reports/stream/_server.js'))
			},
			{
				id: "/api/report",
				pattern: /^\/api\/report\/?$/,
				params: [],
				page: null,
				endpoint: __memo(() => import('./entries/endpoints/api/report/_server.js'))
			}
		],
		prerendered_routes: new Set([]),
		matchers: async () => {
			
			return {  };
		},
		server_assets: {}
	}
}
})();
