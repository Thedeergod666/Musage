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
