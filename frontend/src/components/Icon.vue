<script setup lang="ts">
withDefaults(defineProps<{name:string;size?:number;stroke?:number}>(),{size:20,stroke:1.7});
const paths:Record<string,string>={
 grid:'M3 3h7v7H3zM14 3h7v7h-7zM3 14h7v7H3zM14 14h7v7h-7z',
 chip:'M8 5h8a3 3 0 0 1 3 3v8a3 3 0 0 1-3 3H8a3 3 0 0 1-3-3V8a3 3 0 0 1 3-3ZM9 9h6v6H9zM9 2v3m6-3v3M9 19v3m6-3v3M2 9h3m-3 6h3m14-6h3m-3 6h3',
 search:'m21 21-4.5-4.5M19 11a8 8 0 1 1-16 0 8 8 0 0 1 16 0Z',
 plus:'M12 5v14M5 12h14',close:'m6 6 12 12M6 18 18 6',chevron:'m9 5 7 7-7 7',down:'m6 9 6 6 6-6',
 folder:'M3 7V5a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7Z',
 import:'M12 3v12m-4-4 4 4 4-4M4 16v4a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-4',
 export:'M12 16V4m-4 4 4-4 4 4M4 16v4a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1v-4',
 check:'m5 12 4 4L19 6',settings:'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8Zm-2-5 1-2h2l1 2 2 1 2-1 2 2-1 2 1 2 2 1v3l-2 1-1 2 1 2-2 2-2-1-2 1-1 2h-3l-1-2-2-1-2 1-2-2 1-2-1-2-2-1v-3l2-1 1-2-1-2 2-2 2 1 2-1Z',
 moon:'M21 13A9 9 0 0 1 11 3a9 9 0 1 0 10 10Z',sun:'M12 8a4 4 0 1 0 0 8 4 4 0 0 0 0-8ZM12 2v2m0 16v2M2 12h2m16 0h2M5 5l1.5 1.5m11 11L19 19M5 19l1.5-1.5m11-11L19 5',
 cube:'m12 3 9 5v9l-9 5-9-5V8l9-5Zm0 10v9M3 8l9 5 9-5M7.5 5.5l9 5',
 layers:'m12 3 10 5-10 5L2 8l10-5ZM2 12l10 5 10-5M2 16l10 5 10-5',
 zoomIn:'m21 21-4-4M19 11a8 8 0 1 1-16 0 8 8 0 0 1 16 0ZM7 11h8m-4-4v8',
 zoomOut:'m21 21-4-4M19 11a8 8 0 1 1-16 0 8 8 0 0 1 16 0ZM7 11h8',
 fit:'M8 3H3v5m13-5h5v5M3 16v5h5m8 0h5v-5M8 8h8v8H8z',
 info:'M12 11v6m0-10v.01M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0Z',
 clock:'M12 7v5l3 2M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0Z',
 spark:'m12 2 2.5 7.5L22 12l-7.5 2.5L12 22l-2.5-7.5L2 12l7.5-2.5L12 2Z',
 list:'M9 6h12M9 12h12M9 18h12M3 6h.01M3 12h.01M3 18h.01',
 book:'M12 5C8 2 4 3 2 4v16c4-2 7-1 10 1m0-16c4-3 8-2 10-1v16c-4-2-7-1-10 1V5Z',
 activity:'M2 12h5l3-9 4 18 3-9h5',
 arrow:'M5 12h14m-6-6 6 6-6 6',refresh:'M20 7V2m0 0h-5M20 2l-4 4a8 8 0 1 0 3 10',
 trash:'M3 6h18M9 6V3h6v3M5 6l1 15h12l1-15M10 10v7m4-7v7',
 warning:'m12 3 10 18H2L12 3Zm0 6v5m0 3v.01',file:'M14 2H5v20h14V7l-5-5Zm0 0v6h6M8 13h8m-8 4h5',
 help:'M9 8a3 3 0 1 1 5 3c-1.5.6-2 1.5-2 3m0 3v.01M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0Z',
 pause:'M8 5v14m8-14v14',play:'m8 4 12 8-12 8V4Z',
 eye:'M2 12s4-7 10-7 10 7 10 7-4 7-10 7S2 12 2 12Zm10-3a3 3 0 1 0 0 6 3 3 0 0 0 0-6Z',
 connection:'M5 12a7 7 0 0 1 14 0M8 12a4 4 0 0 1 8 0m-4 3v5m-3 0h6',
 terminal:'m5 7 5 5-5 5m8 0h6',
};
</script>
<template><svg :width="size" :height="size" viewBox="0 0 24 24" fill="none" stroke="currentColor" :stroke-width="stroke" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path :d="paths[name] || paths.chip"/></svg></template>
