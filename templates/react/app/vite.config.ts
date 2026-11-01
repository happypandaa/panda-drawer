import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  // This batch serves the application below its project URL.
  base: './',
  build: { target: 'es2022' },
})
