// Vite 默认支持静态资源 import，但 tsc 不认 *.png/svg。
// 这个文件告诉 tsc 这些模块返回 string（URL）。
declare module "*.png" {
  const src: string;
  export default src;
}
declare module "*.svg" {
  const src: string;
  export default src;
}
declare module "*.jpg" {
  const src: string;
  export default src;
}
declare module "*.jpeg" {
  const src: string;
  export default src;
}

// Vite `?url` 后缀：强制把资源当外部文件 emit（不走 assetsInlineLimit 内联）。
// 默认行为下 <4KB 的资源会被内联成 data: URI，但 Tauri 的 CSP `default-src 'self'`
// 不放行 data:，会导致 <img src="data:..."> 被 block 显示 broken icon。
// 用 ?url 显式走外部文件，保证 dev / prod 行为一致，src 直接是 /assets/xxx.svg。
declare module "*.svg?url" {
  const src: string;
  export default src;
}
declare module "*.png?url" {
  const src: string;
  export default src;
}
