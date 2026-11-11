import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';
export default defineConfig({plugins:[vue()],server:{port:5173,strictPort:true,proxy:{'/api':'http://127.0.0.1:34116'}},build:{target:'es2020',rollupOptions:{output:{manualChunks:{three:['three']}}}}});
