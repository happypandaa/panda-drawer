# Launcher visual assets

These assets implement the Radio Bob inspired design approved by the user in
Figma (`lXWJIfHMpzoLiPV4kB9KMQ`, frame `25:17686`).

- `dots-light.png` and `dots-dark.png` are the original local Radio Bob dot
  textures, copied from the local `radio-bob/public/assets/` directory with the
  user's request to reuse that visual style. No Radio Bob application code,
  project data, or configuration is included.
- `node-mark.png` is the green Node service badge supplied by the user in this
  design conversation. It identifies projects with a local Node.js service;
  it does not indicate whether that service is running.
- `../fonts/doto-latin.woff2` is Doto by The Doto Project Authors. It is used
  unchanged for the wordmark under the SIL Open Font License 1.1; the full
  license is in `../fonts/doto-LICENSE.txt`.
- `../fonts/space-mono-400.woff2` and `../fonts/space-mono-700.woff2` are Space
  Mono by The Space Mono Project Authors. They are used unchanged for small
  interface labels under the SIL Open Font License 1.1; the full license is
  in `../fonts/space-mono-LICENSE.txt`.

The font files and their license texts were copied from the same Radio Bob
static asset directory. Their upstream projects are identified in those
license texts. Interface icons continue to use the existing Lucide dependency;
the application's existing panda artwork remains unchanged.
