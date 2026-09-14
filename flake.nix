{
  description = "Wasm Component Model Polyfill";

  # Katsuobushi carries the Rust build infra (crane, nix-filter, rust-overlay)
  # as transitive inputs, so this flake declares only nixpkgs, flake-utils, and
  # katsuobushi. `nixpkgs.follows` unifies the dependency graph on one nixpkgs.
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    katsuobushi.url = "github:cdata/katsuobushi/v0.5.1";
    katsuobushi.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      katsuobushi,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          # The Rust helper applies rust-overlay internally, so only the
          # katsuobushi overlay (menu helpers) is needed here.
          overlays = [ katsuobushi.overlays.default ];
          config = {
            allowUnfreePredicate =
              pkg:
              builtins.elem (pkgs.lib.getName pkg) [
                "google-chrome"
              ];
          };
        };

        inherit (pkgs.katsuobushi) makeMenu makeDevShellHook;

        # The project board under `project/kanban/`, driven by `katsuctl
        # project` and surfaced as the `project` menu command. The check
        # keeps the board and its card notes consistent.
        project = katsuobushi.lib.project {
          inherit pkgs;
          katsuctl = katsuobushi.packages.${system}.katsuctl;
          workspaceRoot = ./.;
        };

        # Chrome differs by platform: Darwin uses google-chrome (unfree)
        # because chromium is unmaintained there; everything else uses
        # chromium.
        chrome = if pkgs.stdenv.isDarwin then pkgs.google-chrome else pkgs.chromium;
        chromePath = "${chrome}/bin/${chrome.meta.mainProgram}";

        # Headless Chrome refuses to start under the Nix build sandbox on
        # Linux and under the default sandbox/GPU configuration on Darwin.
        # wasm-bindgen-test-runner reads this JSON and passes the flags
        # through ChromeDriver on every platform.
        webdriverConfig = (pkgs.formats.json { }).generate "webdriver.json" {
          "goog:chromeOptions" = {
            binary = chromePath;
            args = [
              "--headless=new"
              "--no-sandbox"
              "--disable-gpu"
              "--disable-dev-shm-usage"
            ];
          };
        };

        # Markdown helpers: one Prettier configuration drives the `markdown`
        # menu command (`format` / `lint` subcommands) and the check below.
        # Scoped to the design corpus and the board README; the board file
        # itself is machine-managed and stays out of the gate.
        markdown = katsuobushi.lib.markdown {
          inherit pkgs;
          workspaceRoot = ./.;
          include = [
            "project/design/**/*.md"
            "project/kanban/README.md"
          ];
          exclude = project.markdownExclude;
        };

        # Tools every Rust derivation in this workspace needs as
        # `nativeBuildInputs`.
        commonBuildInputs = with pkgs; [
          pkg-config
        ];

        # Rust build helpers from katsuobushi, so upstream fixes propagate
        # here without a local copy to maintain. crane, nix-filter, and
        # rust-overlay are inherited from katsuobushi.
        rustHelpers = katsuobushi.lib.rust {
          inherit pkgs;
          workspaceRoot = ./.;
          # Owner-qualified identifier; namespaces the out-of-tree cargo
          # target directory that `rustEnvironmentHook` points cargo at.
          projectId = "cdata/wasm-component-model-polyfill";
          nativeBuildInputs = commonBuildInputs;
          # wasm-bindgen-cli must match the `wasm-bindgen` crate version that
          # Cargo.lock resolves. The helper reads the version from the lock
          # file; these are the fixed-output hashes for it. When the workspace
          # bumps `wasm-bindgen`, add the new version here (bootstrap both
          # fields with `pkgs.lib.fakeHash` and let the failing build report
          # the real values).
          wasmBindgenHashes."0.2.108" = {
            hash = "sha256-UsuxILm1G6PkmVw0I/JF12CRltAfCJQFOaT4hFwvR8E=";
            cargoHash = "sha256-iqQiWbsKlLBiJFeqIYiXo3cqxGLSjNM8SOWXGM9u43E=";
          };
        };

        inherit (rustHelpers)
          buildCrate
          buildTestArchive
          cargoChecks
          checkArtifactAlignment
          rustEnvironmentHook
          rustToolchain
          wasm-bindgen-cli
          ;

        developmentBuildInputs =
          commonBuildInputs
          ++ (with pkgs; [
            cargo-nextest
            chrome
            chromedriver
            rustToolchain
            wasm-bindgen-cli
          ])
          ++ [
            markdown.prettier
            # Diagnostic: compares a Nix-built deps bundle's alignment
            # manifest against the live shell (`katsuobushi-check-artifact-
            # alignment <bundle>`), so a silent full rebuild has a named cause.
            checkArtifactAlignment
          ];

        developmentEnvVars = {
          # wasip3 component instantiation can be slow under headless Chrome;
          # extend the default 20s timeout to match dialog-db's 180s.
          "WASM_BINDGEN_TEST_TIMEOUT" = "180";
          "CHROME_PATH" = chromePath;
          "CHROME" = chromePath;
          "CHROMEDRIVER" = "${pkgs.chromedriver}/bin/chromedriver";
          "WASM_BINDGEN_TEST_WEBDRIVER_JSON" = webdriverConfig;
        };

        # Wraps a Nix-built test archive (`buildTestArchive`) into a menu
        # command. The dev-shell user runs e.g. `tests web debug` and Nix
        # builds (or returns a cache hit for) the archive, then nextest
        # replays the tests against the local workspace via
        # `--workspace-remap`. Anything the operator types after the leaf
        # (`tests native debug --no-fail-fast`) reaches nextest.
        # The conformance progress summary: replays only the summary test
        # from the native debug archive with its output shown, and writes
        # the JSON copy under the cargo target directory.
        conformanceSummaryCommand = ''
          archive=$(nix build --no-link --print-out-paths .#tests-native-debug)
          summary="''${CARGO_TARGET_DIR:-target}/conformance/summary.json"
          mkdir -p "$(dirname "$summary")"
          WCMP_CONFORMANCE_SUMMARY="$summary" cargo nextest run \
            --workspace-remap ./ \
            --archive-file "$archive/tests-native-debug.tar.zst" \
            --no-capture \
            -E 'test(it_reports_conformance_progress)'
        '';

        menuTestCommand =
          {
            description,
            package,
            # Print the conformance progress summary after the run.
            summary ? false,
          }:
          {
            inherit description;
            command = ''
              archive=$(nix build --no-link --print-out-paths .#${package})
              cargo nextest run \
                --workspace-remap ./ \
                --archive-file "$archive/${package}.tar.zst" \
                "$@"
            ''
            + pkgs.lib.optionalString summary conformanceSummaryCommand;
          };

        commands = {
          "build" = {
            description = "Build the polyfill crate for both targets as Nix derivations";
            subcommands = {
              debug = {
                description = "Unoptimized build (prints the store paths)";
                command = ''
                  nix build --no-link --print-out-paths .#polyfill-native-debug .#polyfill-web-debug
                '';
              };
              release = {
                description = "Optimized build (prints the store paths)";
                command = ''
                  nix build --no-link --print-out-paths .#polyfill-native-release .#polyfill-web-release
                '';
              };
            };
          };

          "tests" = {
            description = "Run the test suites from Nix-built archives";
            subcommands = {
              native = {
                description = "Unit and integration tests on ${system}";
                subcommands = {
                  debug = menuTestCommand {
                    description = "Unit and integration tests (${system}, debug)";
                    package = "tests-native-debug";
                    summary = true;
                  };
                  release = menuTestCommand {
                    description = "Unit and integration tests (${system}, release)";
                    package = "tests-native-release";
                    summary = true;
                  };
                };
              };
              web = {
                description = "Unit and integration tests in headless Chrome";
                subcommands = {
                  debug = menuTestCommand {
                    description = "Unit and integration tests (wasm32-unknown-unknown, debug)";
                    package = "tests-web-debug";
                  };
                  release = menuTestCommand {
                    description = "Unit and integration tests (wasm32-unknown-unknown, release)";
                    package = "tests-web-release";
                  };
                };
              };
              conformance = {
                description = "Conformance progress per corpus (native, debug)";
                command = conformanceSummaryCommand;
              };
              all = {
                description = "Every archive, each reported (grab a coffee)";
                command = ''
                  status=0
                  for suite in "native debug" "native release" "web debug" "web release"; do
                    # shellcheck disable=SC2086
                    if tests $suite "$@"; then
                      echo "tests $suite: passed"
                    else
                      echo "tests $suite: FAILED"
                      status=1
                    fi
                  done
                  exit "$status"
                '';
              };
            };
          };

          "lint" = {
            description = "Every check the flake declares (nix flake check)";
            command = "nix flake check";
          };

        }
        // markdown.menuCommands
        // project.menuCommands;

        # The polyfill crate itself, as a derivation per (target, profile).
        # Building an `rlib` installs no binary; the store path holds the
        # build log and proves the crate compiles for the target.
        polyfillCrate =
          {
            target ? null,
            profile,
          }:
          buildCrate {
            pname = "wasm-component-model-polyfill";
            version = "0.1.0";
            cargoExtraArgs = "--package wasm-component-model-polyfill";
            inherit target profile;
          };

        # Bound here so the `workspace-deps-dev` package below can name the
        # same derivation the archive builds against.
        testsNativeDebug = buildTestArchive {
          name = "native-debug";
          profile = "dev";
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
          polyfill-native-debug = polyfillCrate { profile = "dev"; };
          polyfill-native-release = polyfillCrate { profile = "release"; };
          polyfill-web-debug = polyfillCrate {
            target = "wasm32-unknown-unknown";
            profile = "dev";
          };
          polyfill-web-release = polyfillCrate {
            target = "wasm32-unknown-unknown";
            profile = "release";
          };

          # `buildTestArchive` defaults its cargo profile to `release`; the
          # debug archives name `dev` explicitly (Cargo's built-in unoptimized
          # profile) so they are what their names promise.
          tests-native-debug = testsNativeDebug;

          tests-native-release = buildTestArchive {
            name = "native-release";
          };

          tests-web-debug = buildTestArchive {
            name = "web-debug";
            target = "wasm32-unknown-unknown";
            profile = "dev";
          };

          tests-web-release = buildTestArchive {
            name = "web-release";
            target = "wasm32-unknown-unknown";
          };

          # The host-target `dev` dependency closure the debug archive and
          # every other `dev` build compile against. Named so the alignment
          # checker can be pointed at it: `katsuobushi-check-artifact-alignment
          # --profile dev "$(nix build .#workspace-deps-dev --print-out-paths)"`.
          workspace-deps-dev = testsNativeDebug.cargoArtifacts;
        };

        checks =
          cargoChecks
          // markdown.checks
          // project.checks
          // {
            # The doctests are not in a nextest archive (nextest does not run
            # them), so they get a derivation of their own: the workspace's
            # `cargo test --doc` against the `dev` dependency bundle.
            doctests = buildCrate {
              pname = "wasm-component-model-polyfill-doctests";
              version = "0.1.0";
              profile = "dev";
              buildPhaseCargoCommand = "cargo test --doc --workspace";
              installPhaseCommand = "touch $out";
              doInstallCargoArtifacts = false;
              # Nothing to install: the build phase is a test run, not a
              # build, so there is no cargo build log for crane's hook to read.
              doNotPostBuildInstallCargoBinaries = true;
            };
          };

        devShells.default = pkgs.mkShell {
          name = "wcmp";
          env = developmentEnvVars;
          nativeBuildInputs = menu.commands ++ developmentBuildInputs;
          shellHook = rustEnvironmentHook + makeDevShellHook menu;
        };
      }
    );
}
