import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

export default defineConfig({
  plugins: [react()],
  // The platform serves this build below the project URL and retains older assets.
  base: './',
  build: { target: 'es2022' },
})
