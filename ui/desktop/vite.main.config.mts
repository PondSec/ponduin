import { defineConfig } from 'vite';

// https://vitejs.dev/config
export default defineConfig({
  define: {
    'process.env.GITHUB_OWNER': JSON.stringify(process.env.GITHUB_OWNER || 'PondSec'),
    'process.env.GITHUB_REPO': JSON.stringify(process.env.GITHUB_REPO || 'ponduin'),
    'process.env.PONDUIN_BUNDLE_NAME': JSON.stringify(process.env.PONDUIN_BUNDLE_NAME || 'Ponduin'),
  },
});
