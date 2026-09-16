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
            "CLAUDE.md"
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
          buildWasmCrate
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
        # The conformance progress summary on one target: replays only the
        # summary test from the archive with its output shown. The native
        # run also writes the JSON copy under the cargo target directory.
        conformanceSummaryFor = package: ''
          echo "== conformance progress: ${package}"
          archive=$(nix build --no-link --print-out-paths .#${package})
          summary="''${CARGO_TARGET_DIR:-target}/conformance/summary.json"
          mkdir -p "$(dirname "$summary")" "''${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/${package}-summary"
          WCMP_CONFORMANCE_SUMMARY="$summary" cargo nextest run \
            --workspace-remap ./ \
            --archive-file "$archive/${package}.tar.zst" \
            --extract-to "''${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/${package}-summary" \
            --extract-overwrite \
            --no-capture \
            -E 'test(it_reports_conformance_progress)'
          rm -rf "''${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/${package}-summary"
        '';
        conformanceSummaryCommand = conformanceSummaryFor "tests-native-debug";

        # Every replay extracts the archive (4 to 7 GB of test binaries)
        # into a directory on disk under the user's cache directory, not
        # under `$TMPDIR`, which is a RAM-backed tmpfs on most Linux
        # systems, and removes it when nextest exits. The browsers a web
        # lane starts write their profiles under the same directory. The
        # path is kept short on purpose: Chromium puts Unix sockets under
        # `$TMPDIR`, and a socket path longer than 108 bytes aborts the
        # browser at startup.
        testWorkspace = package: ''
          workspace="''${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/${package}"
          rm -rf "$workspace"
          mkdir -p "$workspace/archive" "$workspace/tmp"
          trap 'rm -rf "$workspace"' EXIT
          export TMPDIR="$workspace/tmp"
        '';

        # A web lane runs one headless browser and one test runner (about
        # 2 GB) per test in flight, so its parallelism follows available
        # memory, one test per 4 GB and at most 8, unless the operator
        # sets `NEXTEST_TEST_THREADS` or passes `-j`. Native lanes keep
        # nextest's default, one test per core.
        browserTestThreads = ''
          if [ -z "''${NEXTEST_TEST_THREADS:-}" ]; then
            threads=4
            if [ -r /proc/meminfo ]; then
              available_kb=$(awk '/MemAvailable/ { print $2 }' /proc/meminfo)
              threads=$(( available_kb / 1024 / 1024 / 4 ))
            fi
            [ "$threads" -lt 1 ] && threads=1
            [ "$threads" -gt 8 ] && threads=8
            export NEXTEST_TEST_THREADS="$threads"
            echo "browser tests: $threads at a time (set NEXTEST_TEST_THREADS or pass -j to change)"
          fi
        '';

        menuTestCommand =
          {
            description,
            package,
            # Print the conformance progress summary after the run.
            summary ? false,
            # Cap the parallelism from available memory (web lanes).
            browser ? false,
          }:
          {
            inherit description;
            command = ''
              archive=$(nix build --no-link --print-out-paths .#${package})
            ''
            + testWorkspace package
            + pkgs.lib.optionalString browser browserTestThreads
            + ''
              cargo nextest run \
                --workspace-remap ./ \
                --archive-file "$archive/${package}.tar.zst" \
                --extract-to "$workspace/archive" \
                --extract-overwrite \
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
                description = "Unit and integration tests in headless Chrome (one browser per test in flight; parallelism follows available memory unless NEXTEST_TEST_THREADS or -j says otherwise)";
                subcommands = {
                  debug = menuTestCommand {
                    description = "Unit and integration tests (wasm32-unknown-unknown, debug)";
                    package = "tests-web-debug";
                    browser = true;
                  };
                  release = menuTestCommand {
                    description = "Unit and integration tests (wasm32-unknown-unknown, release)";
                    package = "tests-web-release";
                    browser = true;
                  };
                };
              };
              conformance = {
                description = "Conformance progress per corpus on both targets (debug)";
                command = conformanceSummaryCommand + conformanceSummaryFor "tests-web-debug";
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

          "smoke" = {
            description = "Run the end-to-end smoke test natively or in the browser";
            subcommands = {
              native = {
                description = "Build the smoke test host as a derivation and run it";
                command = ''
                  "$(nix build --no-link --print-out-paths .#smoke-native)"/bin/wcmp-smoke
                '';
              };
              web = {
                description = "Build the smoke test page as a derivation and serve it (uncached, so a rebuilt page shows at once)";
                command = ''
                  site=$(nix build --no-link --print-out-paths .#smoke-web)
                  port="''${1:-8765}"
                  echo "smoke test page: http://127.0.0.1:$port/  (Ctrl-C stops the server)"
                  ${pkgs.python3}/bin/python3 ${./rust/wcmp-smoke/web/serve.py} "$site" "$port"
                '';
              };
              check = {
                description = "Drive the smoke test page headlessly as the flake check does, and print its report";
                command = ''
                  if report=$(nix build --no-link --print-out-paths .#checks.${system}.smoke-web); then
                    cat "$report/report.txt"
                  else
                    echo "smoke check: FAILED (see the build log above)"
                    exit 1
                  fi
                '';
              };
            };
          };

          # The real-guest fixtures under the conformance corpus, rebuilt
          # from their WIT and WAT sources with the flake's pinned
          # `wasm-tools` and `wac`, so a rebuild is byte-for-byte stable
          # until one of the tools is bumped on purpose.
          "fixtures" = {
            description = "Regenerate the conformance fixtures with wasm-tools and wac";
            command = ''
              export PATH=${pkgs.wasm-tools}/bin:${pkgs.wac-cli}/bin:$PATH
              "$(git rev-parse --show-toplevel)"/rust/wasm-component-model-polyfill/tests/corpus/fixtures/build.sh
            '';
          };

        }
        // markdown.menuCommands
        // project.menuCommands;

        # The smoke test host (`rust/wcmp-smoke`): one program that walks
        # the polyfill end to end. Natively it is a binary; for the browser
        # the same binary crate goes through `wasm-bindgen --target web` and
        # ships with its page.
        smokeNative = buildCrate {
          pname = "wcmp-smoke";
          version = "0.1.0";
          cargoExtraArgs = "--package wcmp-smoke";
        };
        smokeWeb = buildWasmCrate {
          pname = "wcmp-smoke-web";
          version = "0.1.0";
          cargoExtraArgs = "--package wcmp-smoke --bin wcmp-smoke";
          doInstallCargoArtifacts = false;
          doNotPostBuildInstallCargoBinaries = true;
          installPhaseCommand = ''
            mkdir -p $out
            $WASM_BINDGEN_BIN --target web --no-typescript \
              --out-dir $out target/wasm32-unknown-unknown/release/wcmp-smoke.wasm
            cp ${./rust/wcmp-smoke/web/index.html} $out/index.html
          '';
        };

        # The web smoke page driven headlessly, as a check: the page
        # `smoke web` serves, loaded in the flake's Chromium through
        # chromedriver inside the build sandbox by `web/check.py`, with
        # its report compared against the native smoke binary's.
        smokeWebCheck =
          pkgs.runCommand "wcmp-smoke-web-check"
            {
              nativeBuildInputs = [
                chrome
                pkgs.chromedriver
                pkgs.python3
              ];
              CHROMEDRIVER = "${pkgs.chromedriver}/bin/chromedriver";
              WASM_BINDGEN_TEST_WEBDRIVER_JSON = webdriverConfig;
            }
            ''
              export HOME=$TMPDIR
              native=$(${smokeNative}/bin/wcmp-smoke | tail -n 1)
              mkdir -p $out
              python3 ${./rust/wcmp-smoke/web/check.py} ${smokeWeb} "$native" $out/report.txt
            '';

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
          # The component toolchain the `fixtures` command runs, exposed so
          # the pinned versions are one `nix build` away.
          wasm-tools = pkgs.wasm-tools;
          wac = pkgs.wac-cli;

          smoke-native = smokeNative;
          smoke-web = smokeWeb;

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
            # The web smoke page must still run: see `smokeWebCheck`.
            smoke-web = smokeWebCheck;
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
