

export const index = 0;
let component_cache;
export const component = async () => component_cache ??= (await import('../entries/fallbacks/layout.svelte.js')).default;
export const imports = ["_app/immutable/nodes/0.SjzItSfM.js","_app/immutable/chunks/C-zbSqNs.js","_app/immutable/chunks/4xzEkbiA.js","_app/immutable/chunks/Ccb5uD3K.js"];
export const stylesheets = [];
export const fonts = [];
