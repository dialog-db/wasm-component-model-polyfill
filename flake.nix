{
  description = "Wasm Component Model Polyfill";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    katsuobushi.url = "github:cdata/katsuobushi";
    crane.url = "github:ipetkov/crane";
    nix-filter.url = "github:numtide/nix-filter";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      katsuobushi,
      crane,
      nix-filter,
      rust-overlay,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [
            (import rust-overlay)
            katsuobushi.overlays.default
          ];
          config = {
            allowUnfreePredicate =
              pkg:
              builtins.elem (pkgs.lib.getName pkg) [
                "google-chrome"
              ];
          };
        };

        filter = nix-filter.lib;

        inherit (pkgs.katsuobushi) makeMenu makeDevShellHook;

        # Chrome differs by platform: Darwin uses google-chrome (unfree)
        # because chromium is unmaintained there; everything else uses
        # chromium.
        chrome = if pkgs.stdenv.isDarwin then pkgs.google-chrome else pkgs.chromium;
        chromePath = "${chrome}/bin/${chrome.meta.mainProgram}";

        # On Darwin, headless Chrome misbehaves under the default sandbox/GPU
        # configuration. wasm-bindgen-test-runner reads this JSON to pass the
        # necessary disable flags through ChromeDriver.
        webdriverConfig = (pkgs.formats.json { }).generate "webdriver.json" {
          "goog:chromeOptions" = {
            binary = chromePath;
            args = [
              "--no-sandbox"
              "--disable-gpu"
            ];
          };
        };

        # Inline rumdl configuration for design-doc formatting. Materialised
        # into the Nix store so the menu command can reference it via
        # `--config` without checking a file into the repo.
        rumdlConfig = (pkgs.formats.toml { }).generate "rumdl.toml" {
          global = {
            "respect-gitignore" = true;

            # The config lives in /nix/store, so rumdl can't write its
            # default cache next to it. Disable rather than redirect.
            cache = false;

            # MD013 (line-length) is noisy on prose; skip it on design docs.
            # disable = [ "MD013" ];
          };

          # Line length
          MD013 = {
            reflow = true;
          };

          # Tables
          MD060 = {
            enabled = true;
          };
        };

        # Tools every Rust derivation in this workspace needs as
        # `nativeBuildInputs`.
        commonBuildInputs = with pkgs; [
          binaryen
          pkg-config
        ];

        rustHelpers = (
          import ./nix/rust.nix {
            inherit pkgs filter crane;
            buildInputs = commonBuildInputs;
            workspaceRoot = ./.;
          }
        );

        inherit (rustHelpers)
          buildWasmCrate
          buildTestArchive
          cargoChecks
          rustToolchain
          wasm-bindgen-cli
          ;

        developmentBuildInputs =
          commonBuildInputs
          ++ (with pkgs; [
            cargo-nextest
            chrome
            chromedriver
            rumdl
            rustToolchain
            wasm-bindgen-cli
          ]);

        developmentEnvVars = {
          # wasip3 component instantiation can be slow under headless Chrome;
          # extend the default 20s timeout to match dialog-db's 180s.
          "WASM_BINDGEN_TEST_TIMEOUT" = "180";
          "CHROME_PATH" = chromePath;
          "CHROME" = chromePath;
          "CHROMEDRIVER" = "${pkgs.chromedriver}/bin/chromedriver";
        }
        // pkgs.lib.optionalAttrs pkgs.stdenv.isDarwin {
          "WASM_BINDGEN_TEST_WEBDRIVER_JSON" = webdriverConfig;
        };

        # Wraps a Nix-built test archive (`buildTestArchive`) into a menu
        # command. The dev-shell user runs e.g. `test:web:debug` and Nix
        # builds (or returns a cache hit for) the archive, then nextest
        # replays the tests against the local workspace via
        # `--workspace-remap`. This is the same pattern PDD001 references.
        menuTestCommand =
          { description, package }:
          {
            inherit description;
            command = ''
              nix build .#${package}
              TESTS_PATH=$(nix eval .#${package}.outPath --raw)
              cargo nextest run \
                --workspace-remap ./ \
                --archive-file "$TESTS_PATH/${package}.tar.zst"
            '';
          };

        # The polyfill's wasm artifact: cargo build → wasm-bindgen → wasm-opt.
        wasm-component-model-polyfill-web = buildWasmCrate {
          pname = "wasm-component-model-polyfill-web";

          buildPhaseCargoCommand = ''
            cargo build \
              --profile wasm-release \
              --package wasm-component-model-polyfill \
              --target wasm32-unknown-unknown

            mkdir -p ./pkg
            "$WASM_BINDGEN_BIN" \
              --target web \
              --out-dir ./pkg \
              --out-name wasm-component-model-polyfill \
              ./target/wasm32-unknown-unknown/wasm-release/wasm_component_model_polyfill.wasm

            "$WASM_OPT_BIN" -Oz \
              -o ./pkg/wasm-component-model-polyfill_bg.wasm \
              ./pkg/wasm-component-model-polyfill_bg.wasm
          '';

          installPhaseCommand = ''
            mkdir -p $out
            cp -r ./pkg/* $out/
          '';

          doInstallCargoArtifacts = false;
        };

        commands = {
          "build" = {
            description = "Produce the Wasm library artifacts and JS bindings (debug)";
            command = ''
              cargo build \
                --package wasm-component-model-polyfill \
                --target wasm32-unknown-unknown
            '';
          };

          "test:native:debug" = menuTestCommand {
            description = "Unit and integration tests (${system}, debug)";
            package = "tests-native-debug";
          };

          "test:native:release" = menuTestCommand {
            description = "Unit and integration tests (${system}, release)";
            package = "tests-native-release";
          };

          "test:web:debug" = menuTestCommand {
            description = "Unit and integration tests (wasm32-unknown-unknown, debug)";
            package = "tests-web-debug";
          };

          "test:web:release" = menuTestCommand {
            description = "Unit and integration tests (wasm32-unknown-unknown, release)";
            package = "tests-web-release";
          };

          "test:all" = {
            description = "Full suite across all configurations (grab a coffee)";
            command = ''
              test:native:debug
              test:native:release
              test:web:debug
              test:web:release
            '';
          };

          "lint" = {
            description = "Clippy and format checks across the workspace";
            command = "nix flake check";
          };

          "format:design" = {
            description = "Format PDD Markdown files in the design/ folder";
            command = ''
              root=$(git rev-parse --show-toplevel)
              rumdl fmt --config ${rumdlConfig} "$root/design"
            '';
          };
        };

        menu = makeMenu {
          title = "WCMP";
          graphic = ''
              -----------------------------
            < Wasm Component Model Polyfill >
              -----------------------------
              \
                \
                  ／l、
                （ﾟ､ ｡ ７
                  l  ~ヽ
                  じしf_,)ノ
          '';
          inherit commands;
        };
      in
      {
        packages = {
          inherit wasm-component-model-polyfill-web;

          tests-native-debug = buildTestArchive {
            name = "native-debug";
          };

          tests-native-release = buildTestArchive {
            name = "native-release";
            args = "--release";
          };

          tests-web-debug = buildTestArchive {
            name = "web-debug";
            target = "wasm32-unknown-unknown";
          };

          tests-web-release = buildTestArchive {
            name = "web-release";
            target = "wasm32-unknown-unknown";
            args = "--release";
          };
        };

        checks = cargoChecks // {
          design = pkgs.runCommand "lint-design" { } ''
            set -e
            ${pkgs.rumdl}/bin/rumdl check --config ${rumdlConfig} ${./.}/design
            touch $out
          '';
        };

        devShells.default = pkgs.mkShell {
          name = "wcmp";
          env = developmentEnvVars;
          nativeBuildInputs = menu.commands ++ developmentBuildInputs;
          shellHook = makeDevShellHook menu;
        };
      }
    );
}
