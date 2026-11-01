# Web application

This project folder is the running application. `panda.json` declares the web entry, optional platform SDK and build script. Source is in `src/`; Vite writes `dist/`.

Close the application window before building. Use the platform Build action, or follow `development.build.script` in `panda.json`. Reopen the window to read the files from this same folder. Failed builds are not rolled back; use Git to save and revert code. See `docs/development.md`.

File data lives in `data/`. Ask the user before deleting it. Browser data is stored separately by the platform and is cleared when the application is removed, even if this folder remains.
