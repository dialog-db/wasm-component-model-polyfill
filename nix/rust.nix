# This module contains helpers for building Rust-based artifacts.
# It exists because we're using [crane](https://crane.dev) to do the building,
# and correct crane usage is somewhat nuanced compared to the built-in Nix
# tools (such as buildRustPackage). Using the helpers here means you can
# maximize the amount of sharing / re-use of dependencies across Rust
# projects.
#
# Adapted from dialog-db's nix/rust.nix
# (https://github.com/dialog-db/dialog-db/blob/main/nix/rust.nix).
{
  pkgs,
  filter,
  crane,
  workspaceRoot,
  buildInputs,
}:

let
  # Cargo dependencies that are Git repositories need to have their expected
  # build hash recorded separately. Crane expects the full git URL as the key.
  cargoGitDependencies = {
    # "git+https://example.com/repo.git?tag=v1.0#<rev>" =
    #   "sha256-...";
  };

  # Filter source to only Rust-relevant files.
  rustSource = filter {
    root = workspaceRoot;
    include = [
      ".cargo"
      "Cargo.lock"
      "Cargo.toml"
      "rust-toolchain.toml"
      "rust"
    ];
  };

  rustToolchain = pkgs.rust-bin.fromRustupToolchainFile (workspaceRoot + "/rust-toolchain.toml");
  craneLib = (crane.mkLib pkgs).overrideToolchain (_: rustToolchain);

  # wasm-bindgen-cli must match the wasm-bindgen crate version used by the
  # workspace exactly, or the generated bindings will fail to load. The pin
  # below MUST track `wasm-bindgen` in the root `Cargo.toml`'s
  # `[workspace.dependencies]` table.
  wasm-bindgen-cli =
    with pkgs;
    buildWasmBindgenCli rec {
      src = fetchCrate {
        pname = "wasm-bindgen-cli";
        version = "0.2.108";
        hash = "sha256-UsuxILm1G6PkmVw0I/JF12CRltAfCJQFOaT4hFwvR8E=";
      };

      cargoDeps = rustPlatform.fetchCargoVendor {
        inherit src;
        inherit (src) pname version;
        hash = "sha256-iqQiWbsKlLBiJFeqIYiXo3cqxGLSjNM8SOWXGM9u43E=";
      };
    };

  # Workspace hygiene: enforces that every crate inherits its dependencies
  # from `[workspace.dependencies]` rather than pinning its own versions.
  # This is the convention PDD001 codifies for the polyfill workspace.
  enforce-workspace-deps =
    with pkgs;
    rustPlatform.buildRustPackage rec {
      pname = "cargo-enforce-shared-workspace-deps";
      version = "0.1.0";
      buildInputs = [ rustToolchain ];

      src = fetchCrate {
        inherit pname version;
        sha256 = "sha256-XOdKeg9tNt/HT+WO9QKtdX3fUMUssVTlXRV0LOIMMzc=";
      };

      cargoHash = "sha256-O6DQXK8/VVwTLuFlSyh8jtBJyAFMfAUNXnTeMWrXTCM=";
    };

  nativeBuildInputs = buildInputs ++ [
    rustToolchain
  ];

  commonAttributes = {
    src = rustSource;
    strictDeps = true;
    inherit nativeBuildInputs;
    buildInputs =
      with pkgs;
      lib.optionals stdenv.isLinux [
        dbus
      ];

    # Git dependencies with hashes for offline evaluation. Crane will
    # automatically find Cargo.lock from src.
    outputHashes = cargoGitDependencies;
    doCheck = false;
  };

  # Build native dependencies once for entire workspace.
  nativeArtifacts = craneLib.buildDepsOnly (
    commonAttributes
    // {
      pname = "wasm-component-model-polyfill-workspace-deps";
    }
  );

  wasmAttributes = commonAttributes // {
    CARGO_BUILD_TARGET = "wasm32-unknown-unknown";
  };

  wasmArtifacts = craneLib.buildDepsOnly (
    wasmAttributes
    // {
      pname = "wasm-component-model-polyfill-workspace-wasm-deps";
    }
  );

  buildCrate =
    attributes:
    craneLib.buildPackage (
      commonAttributes
      // {
        version = "0.1.0";
        cargoArtifacts = nativeArtifacts;
      }
      // attributes
    );

  buildWasmCrate =
    attributes:
    craneLib.buildPackage (
      wasmAttributes
      // {
        cargoArtifacts = wasmArtifacts;

        # These *_BIN envvars are conventional and consumed by build scripts
        # such as `worker-build`; they are also a convenient way to surface
        # the pinned tools to a custom buildPhase.
        WASM_OPT_BIN = "${pkgs.binaryen}/bin/wasm-opt";
        WASM_BINDGEN_BIN = "${wasm-bindgen-cli}/bin/wasm-bindgen";
        ESBUILD_BIN = "${pkgs.esbuild}/bin/esbuild";
      }
      // attributes
    );

  buildTestArchive =
    {
      name,
      args ? "",
      target ? null,
    }:
    let
      targetAttributes = if target == "wasm32-unknown-unknown" then wasmAttributes else commonAttributes;

      targetArtifacts = if target == "wasm32-unknown-unknown" then wasmArtifacts else nativeArtifacts;
    in
    craneLib.mkCargoDerivation (
      targetAttributes
      // {
        pname = "tests-${name}";
        cargoArtifacts = targetArtifacts;

        buildPhaseCargoCommand = ''
          cargo nextest archive \
            ${args} \
            --archive-file ./tests-${name}.tar.zst
        '';

        installPhaseCommand = ''
          mkdir -p $out
          cp ./*.tar.zst $out/
        '';

        doInstallCargoArtifacts = false;
        nativeBuildInputs = (targetAttributes.nativeBuildInputs or [ ]) ++ [ pkgs.cargo-nextest ];
      }
    );

  cargoChecks = {
    clippy = craneLib.cargoClippy (
      commonAttributes
      // {
        pname = "wasm-component-model-polyfill-cargo-clippy-check";
        cargoArtifacts = nativeArtifacts;
        cargoClippyExtraArgs = "--all-targets --all-features -- -D warnings";
      }
    );

    rustfmt = craneLib.cargoFmt {
      src = rustSource;
      pname = "wasm-component-model-polyfill-cargo-fmt-check";
    };

    sharedWorkspaceDeps = buildCrate {
      pname = "shared-workspace-deps-check";
      buildPhase = ''
        ${enforce-workspace-deps}/bin/cargo-enforce-shared-workspace-deps
      '';
      installPhase = ''
        touch $out
      '';
    };
  };
in
{
  inherit
    buildCrate
    buildWasmCrate
    buildTestArchive
    rustToolchain
    cargoChecks
    wasm-bindgen-cli
    ;
}
