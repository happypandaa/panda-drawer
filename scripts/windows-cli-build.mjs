// Tauri statically links its manager CRT by default. The separately built
// Windows CLI needs the same policy so clean systems need no extra VC runtime.
// Cargo gives CARGO_ENCODED_RUSTFLAGS precedence over space-separated RUSTFLAGS.
export function windowsCliBuildEnvironment(environment) {
  const encoded = environment.CARGO_ENCODED_RUSTFLAGS;
  const flags = encoded !== undefined
    ? (encoded === '' ? [] : encoded.split('\x1f'))
    : (environment.RUSTFLAGS ?? '').split(/\s+/).filter(Boolean);
  return {
    ...environment,
    CARGO_ENCODED_RUSTFLAGS: [...flags, '-C', 'target-feature=+crt-static'].join('\x1f'),
  };
}
