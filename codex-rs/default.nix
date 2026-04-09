{
  cmake,
  fetchurl,
  llvmPackages,
  openssl,
  libcap ? null,
  rustPlatform,
  pkg-config,
  lib,
  stdenv,
  version ? "0.0.0",
  ...
}:
let
  # Pre-fetch the rusty_v8 static library required by the v8 crate.
  # The v8 crate's build.rs tries to download this at build time, which
  # fails in the Nix sandbox (no network). We fetch it ahead of time
  # and point RUSTY_V8_ARCHIVE at it.
  rustyV8Version = "146.4.0";
  rustyV8 = {
    "aarch64-darwin" = {
      target = "aarch64-apple-darwin";
      hash = "sha256-v+LJvjKlbChUbw+WWCXuaPv2BkBfMQzE4XtEilaM+Yo=";
    };
    "x86_64-darwin" = {
      target = "x86_64-apple-darwin";
      hash = "sha256-YwzSQPG77NsHFBfcGDh6uBz2fFScHFFaC0/Pnrpke7c=";
    };
    "x86_64-linux" = {
      target = "x86_64-unknown-linux-gnu";
      hash = "sha256-5ktNmeSuKTouhGJEqJuAF4uhA4LBP7WRwfppaPUpEVM=";
    };
    "aarch64-linux" = {
      target = "aarch64-unknown-linux-gnu";
      hash = "sha256-2/FlsHyBvbBUvARrQ9I+afz3vMGkwbW0d2mDpxBi7Ng=";
    };
  }.${stdenv.hostPlatform.system} or (throw "Unsupported platform for rusty_v8: ${stdenv.hostPlatform.system}");
  rustyV8Archive = fetchurl {
    url = "https://github.com/denoland/rusty_v8/releases/download/v${rustyV8Version}/librusty_v8_release_${rustyV8.target}.a.gz";
    hash = rustyV8.hash;
  };
in
rustPlatform.buildRustPackage (_: {
  env.PKG_CONFIG_PATH = lib.makeSearchPathOutput "dev" "lib/pkgconfig" (
    [ openssl ] ++ lib.optionals stdenv.isLinux [ libcap ]
  );
  env.RUSTY_V8_ARCHIVE = rustyV8Archive;
  pname = "codex-rs";
  inherit version;
  cargoLock.lockFile = ./Cargo.lock;
  doCheck = false;
  src = ./.;

  # Patch the workspace Cargo.toml:
  # 1. Set version from Nix (for dev builds)
  # 2. Exclude v8-poc from workspace (experimental, not needed for CLI)
  postPatch = ''
    sed -i 's/^version = "0\.0\.0"$/version = "${version}"/' Cargo.toml
    sed -i '/"v8-poc",/d' Cargo.toml
    sed -i '/"codex-v8-poc",/d' Cargo.toml
    sed -i '/codex-v8-poc/d' Cargo.toml
  '';
  nativeBuildInputs = [
    cmake
    llvmPackages.clang
    llvmPackages.libclang.lib
    openssl
    pkg-config
  ] ++ lib.optionals stdenv.isLinux [
    libcap
  ];

  cargoLock.outputHashes = {
    "ratatui-0.29.0" = "sha256-HBvT5c8GsiCxMffNjJGLmHnvG77A6cqEL+1ARurBXho=";
    "crossterm-0.28.1" = "sha256-6qCtfSMuXACKFb9ATID39XyFDIEMFDmbx6SSmNe+728=";
    "nucleo-0.5.0" = "sha256-Hm4SxtTSBrcWpXrtSqeO0TACbUxq3gizg1zD/6Yw/sI=";
    "nucleo-matcher-0.3.1" = "sha256-Hm4SxtTSBrcWpXrtSqeO0TACbUxq3gizg1zD/6Yw/sI=";
    "runfiles-0.1.0" = "sha256-uJpVLcQh8wWZA3GPv9D8Nt43EOirajfDJ7eq/FB+tek=";
    "tokio-tungstenite-0.28.0" = "sha256-hJAkvWxDjB9A9GqansahWhTmj/ekcelslLUTtwqI7lw=";
    "tungstenite-0.27.0" = "sha256-AN5wql2X2yJnQ7lnDxpljNw0Jua40GtmT+w3wjER010=";
  };

  meta = with lib; {
    description = "OpenAI Codex command‑line interface rust implementation";
    license = licenses.asl20;
    homepage = "https://github.com/openai/codex";
    mainProgram = "codex";
  };
})
