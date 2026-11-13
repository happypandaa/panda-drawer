---
title: panda.json reference
description: Fields, configuration, localization, and agent declarations in panda.json v2, with three complete examples.
lang: en
copy_code: true
---

# panda.json reference

Updated 2026-11-13. Sources include the [platform manifest reference](https://github.com/happypandaa/panda-drawer/blob/develop/skills/local-projects-app/references/panda-json.md), [JSON Schema](https://github.com/happypandaa/panda-drawer/blob/develop/schemas/panda.schema.json), [manifest validation](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/panda_manifest.rs), and [configuration field validation](https://github.com/happypandaa/panda-drawer/blob/develop/crates/platform-core/src/panda_manifest/configuration.rs).

The `panda.json` file identifies the app, its web entry point, and any backend or agent instructions. Store it in the app root with the source files.

## File location and fields

- Use UTF-8 JSON and set `manifestVersion` to `2`. Update older manifests to the current format. The platform rejects `runtime`, unknown fields, duplicate keys, invalid types, and explicit `null`.
- The manifest declares the app structure and available settings. The platform records instance IDs, ownership, saved configuration values, secrets, and logs.
- Use `/` as the path separator. An entry is relative to its corresponding root. Other paths are relative to the app root. Avoid absolute paths, paths that escape the app, and links to external locations. A web root cannot point to `data/`, `node_modules/`, or hidden content.
- Validate changes against the shared schema. The platform also checks file existence, path boundaries, and references between fields.

| Field | Required | Meaning |
| --- | --- | --- |
| `manifestVersion` | Yes | `2` |
| `$schema` | No | Editor hint. The platform does not fetch it. |
| `app.name` | Yes | Name shown on the app card and in the window title |
| `app.version`, `app.description`, `app.icon` | No | Version, summary, and path to a PNG icon in the app folder |
| `web.root`, `web.entry` | Yes | The web root can be `.`. The entry is relative to the web root. |
| `backend` | No | Declares a backend. Set `root`, `entry`, `nodeMajor: 24`, and `protocolVersion: 1`. |
| `backend.apiPrefixes` | No | Full path-segment prefixes for app endpoints. Defaults to `/api` and `/tts`. Use `[]` to disable proxying. The root path, prefixes starting with `/_platform`, encoded paths, and dot paths are forbidden. |
| `platform.sdkApiVersion` | No | Set to `1`. Any app can enable the SDK. |
| `development` | No | `sourceDir` defaults to `.`. Set `build.kind` to `npm` and `build.script` to an existing npm script. This builds the web interface only. |
| `configuration` | No | User configuration fields. `secret: true` requires `backend`. |
| `documentation` | No | `readme`, `development`, and `database`. Declared files must exist. |
| `agent.instructions` | No | Path to an existing non-hidden regular file inside the app, such as `AGENTS.md` |
| `window` | No | Both `width` and `height` are required when declared. `rememberSize` defaults to `true`. |
| `runtimeRequirements` | No | External addresses and device purposes for the web interface. The backend's network requests are independent of this policy. |
| `localization` | No | Overrides platform display text for `zh-CN` and `en`. |

If web output is missing, the build declaration determines whether the platform can prepare it. The backend entry and dependencies must be ready ahead of time. Paths in `agent.instructions` cannot include empty segments, `.`, `..`, backslashes, colons, or hidden directories. Agent and backend declarations are independent. The platform uses `isAgent` for display and continues to validate file access against the manifest and path boundaries.

## User configuration

Set `configuration.version` to `1`. The `fields` array can have up to 32 entries.

| Field | Required | Meaning |
| --- | --- | --- |
| `name` | Yes | Unique environment variable name. Start with an uppercase English letter and use uppercase letters, digits, and underscores only. Avoid names reserved by the platform or operating system. |
| `label` | Yes | Name displayed in the form |
| `type` | Yes | `string`, `url`, `integer`, `boolean`, or `select` |
| `required` | Yes | Whether the user must enter a value |
| `secret` | Yes | Secret fields must have type `string` and require `backend`. |
| `description` | Yes | Purpose of the setting |
| `howToGet` | Yes | An array of 1 to 12 nonempty strings explaining how to obtain the value |
| `placeholder` | No | Input hint |
| `default` | No | Must match the field type. Secret fields cannot have defaults. A `url` default must be a valid HTTPS address, and a `select` default must match one of the options. |
| `helpUrl` | No | HTTPS URL with no credentials, query, or fragment |
| `options` | For `select` only | 1 to 32 `value` and `label` pairs with unique values. Other types cannot declare options. |
| `endpointField` | No | A secret field can reference a public `url` field in the same declaration to associate the key with a service address. |

The user enters values in the platform form. The web interface can read public values through `/_platform/config.json` or `panda.config()`. Strings, numbers, and booleans retain their types. Unset optional values are omitted. At startup, the backend receives configuration through environment variables named by each field's `name`. Secrets are injected only into the corresponding backend.

## Platform display text and app language

| Localizable object | Keys and supported fields |
| --- | --- |
| `app` | `name`, `description` |
| `configuration` | Keyed by the original field `name`. Supports `label`, `description`, `placeholder`, `howToGet`, and `options`. Option labels are keyed by the original `value`. |
| `runtimeRequirements.network` | Keyed by `connect`, `images`, or `media`, then by the original `origin`. Supports `purpose` and `dataSent`. |
| `runtimeRequirements.devices` | Supports `purpose` for declared `camera` and `microphone` entries. |

- Group `localization` by `zh-CN` and `en`. Overrides apply field by field, with missing translations falling back to the original text. A `howToGet` override replaces the entire nonempty step array. The platform rejects invalid references, empty strings, explicit `null`, and unknown keys.
- Overrides apply to the platform interface and native window titles. Configuration values, defaults, permissions, paths, and original app information returned by the SDK come from the base declaration.
- Changing the platform language updates display text already loaded. The app can follow `navigator.language` or offer its own setting. The platform does not send its language setting through the SDK, environment variables, or notifications. Reload after editing the manifest.

## Reloading after changes

- Prepare the runtime files and save pending input. Then click **Reload** in the title bar or run `app reload --app <id>`. This opens the app if it is closed.
- If the manifest is invalid or required configuration is missing, the platform keeps the running instance. A failed startup switch displays an error. Fix the issue and reload again.
- To add a backend, declare `backend` and prepare the runtime files. To remove it, also remove all secret configuration declarations. The platform deletes the old secrets after a successful switch and keeps them if the switch fails.
- Adding or removing a backend preserves the app ID, address, folder, `data/`, public configuration, and browser data. A successful switch invalidates old sessions.

## Example 1. Minimal web app

Place `index.html` in the app root.

```json
{
  "manifestVersion": 2,
  "app": { "name": "Pomodoro Timer" },
  "web": { "root": ".", "entry": "index.html" }
}
```

## Example 2. Agent app with the SDK and a web build

Place `AGENTS.md` in the app root. The `build` script in `package.json` creates `dist/index.html`. Include a matching npm lockfile.

```json
{
  "manifestVersion": 2,
  "app": { "name": "Reading Notes" },
  "web": { "root": "dist", "entry": "index.html" },
  "platform": { "sdkApiVersion": 1 },
  "development": {
    "sourceDir": ".",
    "build": { "kind": "npm", "script": "build" }
  },
  "agent": { "instructions": "AGENTS.md" }
}
```

## Example 3. Backend, secret settings, and display translations

Place the backend in `backend/server.mjs` and the web entry in `web/index.html`. The user enters their own key in the platform form. The base display text is in Chinese, and `localization.en` overrides selected labels for English.

```json
{
  "manifestVersion": 2,
  "app": { "name": "朗读助手", "version": "1.0.0" },
  "web": { "root": "web", "entry": "index.html" },
  "backend": {
    "root": "backend",
    "entry": "server.mjs",
    "nodeMajor": 24,
    "protocolVersion": 1,
    "apiPrefixes": ["/v1"]
  },
  "platform": { "sdkApiVersion": 1 },
  "configuration": {
    "version": 1,
    "fields": [
      {
        "name": "SERVICE_URL",
        "label": "服务地址",
        "type": "url",
        "required": true,
        "secret": false,
        "description": "接收朗读请求的服务地址。",
        "howToGet": ["按服务商说明填写 HTTPS 接口地址。"],
        "default": "https://api.example.com"
      },
      {
        "name": "SERVICE_API_KEY",
        "label": "服务 API Key",
        "type": "string",
        "required": true,
        "secret": true,
        "description": "调用朗读服务的个人密钥。",
        "howToGet": ["在服务商控制台创建 API Key。"],
        "endpointField": "SERVICE_URL"
      },
      {
        "name": "VOICE",
        "label": "朗读风格",
        "type": "select",
        "required": false,
        "secret": false,
        "description": "选择应用提供的朗读风格。",
        "howToGet": ["选择喜欢的风格。"],
        "default": "calm",
        "options": [
          { "value": "calm", "label": "平静" },
          { "value": "bright", "label": "明快" }
        ]
      }
    ]
  },
  "localization": {
    "en": {
      "app": { "name": "Read Aloud" },
      "configuration": {
        "SERVICE_URL": { "label": "Service URL" },
        "VOICE": { "label": "Voice", "options": { "calm": "Calm", "bright": "Bright" } }
      }
    }
  }
}
```

[Platform rules](/en/wiki/platform-rules/) · [App runtime](/en/wiki/app-types/) · [Data and configuration](/en/wiki/data-and-config/) · [Adding apps](/en/wiki/adding-apps/)
