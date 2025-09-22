{ pkgs, monorep-deps ? [], ... }:
let
  env = {
    PKG_CONFIG_PATH = "${pkgs.openssl.dev}/lib/pkgconfig:$PKG_CONFIG_PATH";
  };
  rustToolchain = pkgs.rust-bin.nightly.latest.default;
in
rec {
  package = (pkgs.makeRustPlatform {
    cargo = rustToolchain;
    rustc = rustToolchain;
  }).buildRustPackage {
    inherit env;
    pname = "codex-rs";
    version = "0.1.0";
    cargoLock = {
      lockFile = ./Cargo.lock;
      outputHashes = {
        "ratatui-0.29.0" = "sha256-HBvT5c8GsiCxMffNjJGLmHnvG77A6cqEL+1ARurBXho=";
      };
    };
    doCheck = false;
    src = ./.;
    nativeBuildInputs = with pkgs; [
      pkg-config
      openssl
    ];
    meta = with pkgs.lib; {
      description = "OpenAI Codex command‑line interface rust implementation";
      license = licenses.asl20;
      homepage = "https://github.com/openai/codex";
    };
  };
  devShell = pkgs.mkShell {
    inherit env;
    name = "codex-rs-dev";
    packages = monorep-deps ++ [
      rustToolchain
      package
    ];
    shellHook = ''
      echo "Entering development shell for codex-rs"
      alias codex="cd ${package.src}/tui; cargo run; cd -"
      ${rustToolchain}/bin/cargo --version
    '';
  };
  app = {
    type = "app";
    program = "${package}/bin/codex";
  };
}
