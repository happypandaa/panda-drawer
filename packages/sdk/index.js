/** Optional ESM alias: load /_platform/sdk.js in HTML before this module. */
if (!globalThis.panda) throw new Error('Load /_platform/sdk.js before importing @local-projects/sdk.');
export const platform = globalThis.panda;
export const PlatformError = platform.PlatformError;
