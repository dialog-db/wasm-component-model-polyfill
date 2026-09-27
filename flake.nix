{
  description = "Wasm Component Model Polyfill";

  # Katsuobushi carries the Rust build infra (crane, nix-filter, rust-overlay)
  # and the sandbox infra (microvm.nix) as transitive inputs, so this flake
  # declares only nixpkgs, flake-utils, and katsuobushi, plus the project-data
  # sources the sandbox guest carries. `nixpkgs.follows` keeps katsuobushi and
  # everything it builds on this flake's nixpkgs.
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    katsuobushi.url = "github:cdata/katsuobushi/v0.5.1";
    katsuobushi.inputs.nixpkgs.follows = "nixpkgs";

    # The agent harness that runs inside a sandbox VM. Pre-built upstream, so
    # it needs no unfree allowance, and newer than the nixpkgs build. It keeps
    # its own nixpkgs: its packages track a newer nixpkgs than this flake
    # pins, and only the guest's harness closure is built from it.
    llm-agents.url = "github:numtide/llm-agents.nix";

    # Project-data sources for the sandbox guest. `flake = false` fetches the
    # tree only; `nix flake update` moves the pins. The two reference repos
    # are the ones a design review cites side by side with this crate; the
    # config repo holds the owner's universal agent rules.
    component-model-src = {
      url = "github:WebAssembly/component-model";
      flake = false;
    };
    wasmtime-src = {
      url = "github:bytecodealliance/wasmtime";
      flake = false;
    };
    # A private repo, so it is fetched over SSH with the host's keys. Only
    # the tree matters, hence the shallow clone.
    nixos-config = {
      url = "git+ssh://git@github.com/cdata/nixos-config.git?shallow=1";
      flake = false;
    };

    # The browser test runner, pinned by commit. wbg-pool is a member of the
    # dialog-db workspace and inherits that workspace's dependency table, so
    # the whole tree is fetched and one crate is built from it (see
    # `wbg-pool` below). Its codegen library must match the `wasm-bindgen`
    # version Cargo.toml pins; the two move together.
    wbg-pool-src = {
      url = "github:dialog-db/dialog-db/7b84dcac9520f368b6c0714c6e2785e5740f28c7";
      flake = false;
    };

    # The Zena toolchain the Zena scenarios compile with. It tracks Zena's
    # `main`, since Zena has no releases, and the lock holds the pin; `nix
    # flake update zena` moves it. It keeps its own nixpkgs, as Zena's author
    # builds and tests it: a different Node.js or Rust can break its build
    # for reasons unrelated to Zena. Only its `zena` command is used, and
    # only inside the scenario build (see `buildZenaScenarios` below); it
    # never enters the development shell.
    zena.url = "github:elematic/zena";
  };

  outputs =
    {
      self,
      nixpkgs,
      flake-utils,
      katsuobushi,
      llm-agents,
      component-model-src,
      wasmtime-src,
      nixos-config,
      wbg-pool-src,
      zena,
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

        # The sandbox guest is a Linux microvm, so the sandbox app, its
        # checks, and the lifecycle commands exist on Linux only.
        isLinux = pkgs.stdenv.isLinux;

        # The agent sandbox: a hermetic microvm guest that a delegated agent
        # works in, with its blast radius bounded by the VM. `sandbox status`
        # is the host preflight; `sandbox start --agent --name <name>` boots
        # an instance, and its work comes back as a pushed `sandbox/<name>`
        # branch. Inside, the agent goes through `nix develop` and the menu
        # exactly as on the host; `importHostStoreDb` (on by default) lets it
        # reuse every derivation the host has already built, offline.
        sandbox = katsuobushi.lib.sandbox {
          inherit pkgs;
          workspaceRoot = ./.;
          projectId = "cdata/wasm-component-model-polyfill";

          # Beyond the Anthropic+Nix baseline, the guest reaches the cargo
          # registry (a dependency bump made inside the VM), the Rust dist
          # server (a toolchain the host has not built yet), and docs.rs
          # (the preferred place to read a dependency's API).
          allowedOrigins = [
            "static.rust-lang.org"
            "crates.io"
            "index.crates.io"
            "static.crates.io"
            "docs.rs"
          ];

          # The agent harness and nothing else: every build and test tool
          # arrives through `nix develop`, exactly as on the host.
          packages = [ llm-agents.packages.${system}.claude-code ];

          # The host variable is deliberately not CLAUDE_CODE_OAUTH_TOKEN: a
          # Claude Code session orchestrating the sandbox scrubs that name
          # from its children. On the host:
          #   export HARNESS_OAUTH_TOKEN="$(claude setup-token)"
          secrets.CLAUDE_CODE_OAUTH_TOKEN.fromEnv = "HARNESS_OAUTH_TOKEN";

          # Writable, pinned copies of the two references a design review
          # cites, at the paths the owner's rules expect them.
          extraRepos = [
            {
              source = component-model-src;
              dest = "Git/github.com/WebAssembly/component-model";
            }
            {
              source = wasmtime-src;
              dest = "Git/github.com/bytecodealliance/wasmtime";
            }
          ];

          # The owner's universal agent rules, read-only, so the guest agent
          # works under the same global instructions as a host session.
          homeFiles.".claude/CLAUDE.md" = {
            source = nixos-config;
            path = "AGENTS.md";
            mode = "immutable";
          };

          # Untracked, project-local Claude Code configuration (a
          # `.claude/settings.json` pins the in-guest model, for example).
          # Carried when present; a missing path is skipped at launch.
          workspaceContext = [ ".claude" ];

          # Resources, sized around the browser lanes. `tests web` starts one
          # headless Chromium plus one test runner (about 2 GB) per test in
          # flight and caps that from MemAvailable at one per 4 GB, so 16 GB
          # gives a guest three at a time while a Rust build keeps 8 cores.
          # An out-of-memory condition inside this budget takes down the VM,
          # not the host session.
          vcpu = 8;
          mem = 16384;

          # Disk, in MiB. A test replay extracts a 4-7 GB archive under
          # $XDG_CACHE_HOME, which the guest keeps on the scratch volume;
          # rebuilding the archives after a code change adds several GB of
          # dependency bundles and archives to the store overlay and their
          # build trees to the scratch volume. The images are sparse, so the
          # caps cost nothing until used.
          storeVolumeSize = 65536;
          scratchVolumeSize = 131072;

          guestModules = [
            {
              # One derivation at a time inside the guest: `lint` (`nix flake
              # check`) would otherwise start every cargo check at once and
              # exhaust the VM's memory.
              nix.settings.max-jobs = 1;

              # The guest routes every process through its egress proxy with
              # `HTTP_PROXY` and friends. The browser lane talks to ChromeDriver
              # and the test page over loopback, and wasm-bindgen-test-runner's
              # HTTP client (ureq 3) obeys those variables, so without a bypass
              # the proxy answers its loopback CONNECT with 403 and every web
              # test fails. Loopback never needs the allowlist.
              environment.variables = {
                NO_PROXY = "localhost,127.0.0.1,::1";
                no_proxy = "localhost,127.0.0.1,::1";
              };
            }
          ];
        };

        # `wac` composes the fixtures. The pinned nixpkgs carries 0.10.0,
        # which is wrong for that in two ways. It encodes a composition
        # whose socket names a type from an interface it imports (WIT's
        # `use` at world level) as a root-level type import with an `eq`
        # bound — a form the component model does not allow, so the
        # composed component fails validation. And `wac plug` iterated
        # its plugs through a `std` `HashMap`, whose order Rust
        # randomises per run, so a composition with more than one plug
        # was not byte-stable; upstream `1171a94`, "Make `wac plug`
        # output deterministic", fixed that after 0.10.1. 0.11.0 carries
        # both fixes, so it is built here from its own source until
        # nixpkgs catches up.
        wac-cli = pkgs.rustPlatform.buildRustPackage (final: {
          pname = "wac-cli";
          version = "0.11.0";
          src = pkgs.fetchFromGitHub {
            owner = "bytecodealliance";
            repo = "wac";
            tag = "v${final.version}";
            hash = "sha256-nNFdEU0T7Zf9q3boozA0fU+9sCxhnUGf/azVtNesIac=";
          };
          cargoHash = "sha256-seLmdg6p644E/XqyCqTGjufMuXkR9PBtoEvjrp664Go=";
          meta.mainProgram = "wac";
        });

        # The Zena toolchain at the pin, as Zena's own flake builds it. The
        # public binary cache does not hold it, so a cold store builds the
        # whole Zena monorepo once per pin.
        zenaToolchain = zena.packages.${system}.zena;

        # The revision of the `zena` input, which later steps compare with
        # the pin the Zena record names. An input with no revision, such
        # as a `path:` override onto a local checkout, has no pin to
        # compare, so evaluation stops and says so.
        zenaRevision =
          zena.rev or (throw ''
            The zena input has no revision, so the Zena scenarios have no pin to
            record. Override it with a committed Git revision, for example
            --override-input zena git+file:///path/to/zena?rev=<commit>, rather
            than a path: or a dirty tree.
          '');

        # The revision `flake.lock` pins the `zena` input at.
        zenaLockedRevision =
          let
            lock = builtins.fromJSON (builtins.readFile ./flake.lock);
          in
          lock.nodes.${lock.nodes.${lock.root}.inputs.zena}.locked.rev;

        # The script that compiles a directory of scenarios. Its header
        # states the layout of a scenario and of the output.
        zenaScenarioBuilder = pkgs.writeShellApplication {
          name = "zena-build-scenarios";
          runtimeInputs = [
            pkgs.coreutils
            pkgs.gnused
          ];
          text = builtins.readFile ./rust/wasm-component-model-polyfill/tests/zena/build.sh;
        };

        # Compiles every scenario under `scenarios` with the `zena` command
        # of the pinned toolchain, and nothing else from its package. The
        # derivation succeeds when Zena refuses a program: it keeps Zena's
        # exit status and output instead. `$out/zena-revision` and the
        # `zenaRevision` attribute carry the input's revision to the steps
        # that read the output.
        buildZenaScenarios =
          { name, scenarios }:
          pkgs.runCommand name
            {
              ZENA = pkgs.lib.getExe zenaToolchain;
              passthru = { inherit zenaRevision; };
            }
            ''
              # zena-cli runs the compiler under Wasmtime, which looks for a
              # cache directory under $HOME.
              export HOME=$TMPDIR
              ${pkgs.lib.getExe zenaScenarioBuilder} ${scenarios} "$out"
              echo ${zenaRevision} > "$out/zena-revision"
            '';

        zenaScenarios = buildZenaScenarios {
          name = "zena-scenarios";
          scenarios = ./rust/wasm-component-model-polyfill/tests/zena/scenarios;
        };

        # The scenario build against its own cases, failing the check when
        # one does not hold:
        #
        # - A program Zena refuses still builds, and keeps a non-zero
        #   status and Zena's error output but no component.
        # - The build passes a scenario's `wit/` world to Zena. One program
        #   imports a function only its world declares, so it compiles only
        #   with the world. Another lacks an export its world declares, so
        #   Zena refuses it only with the world.
        # - The recorded revision is the one `flake.lock` pins.
        # - A directory with no scenario, or a scenario with no program,
        #   fails the build instead of yielding an empty output. These run
        #   the script alone, with a stand-in for Zena that never runs.
        zenaScenarioBuildCheck =
          let
            built = buildZenaScenarios {
              name = "zena-scenario-build-cases";
              scenarios = ./rust/wasm-component-model-polyfill/tests/zena/build-check;
            };
          in
          pkgs.runCommand "zena-scenario-build-check" { } ''
            refused=${built}/refused-program
            test "$(cat $refused/refused.status)" != 0
            grep -q '^refused.zena:[0-9]*:[0-9]* - Error' $refused/refused.log
            test ! -e $refused/refused.wasm

            declared=${built}/declared-world
            test "$(cat $declared/declared.status)" = 0
            test -s $declared/declared.wasm
            test "$(cat $declared/undeclared-export.status)" != 0
            grep -q "The declared world exports 'missing'" \
              $declared/undeclared-export.log
            test ! -e $declared/undeclared-export.wasm

            test "$(cat ${built}/zena-revision)" = ${zenaLockedRevision}

            export ZENA=${pkgs.coreutils}/bin/false
            mkdir -p no-scenario no-program/empty
            touch no-scenario/README.md no-program/empty/README.md
            for layout in no-scenario no-program; do
              if ${pkgs.lib.getExe zenaScenarioBuilder} $layout $layout.out \
                2>$layout.err; then
                echo "the build accepted a directory with $layout" >&2
                exit 1
              fi
            done
            grep -q 'no-scenario holds no scenario directory' no-scenario.err
            test ! -e no-scenario.out
            grep -q 'scenario empty holds no .zena program' no-program.err

            touch "$out"
          '';

        # The Wasmtime run of the scenarios (`rust/wcmp-wasmtime`): a
        # native program built against this workspace's Wasmtime, the one
        # the polyfill's native backend links, with `wasmtime-wasi` at the
        # same version. Zena's own flake pins another Wasmtime, which only
        # the compile step above runs, inside Zena's command; it never
        # runs a scenario.
        wasmtimeRunner = buildCrate {
          pname = "wcmp-wasmtime";
          version = "0.1.0";
          cargoExtraArgs = "--package wcmp-wasmtime";
        };

        # Runs every compiled scenario through Wasmtime, before either
        # polyfill subject, and writes `$out/<scenario>/observations.txt`:
        # the Wasmtime stage on the first line, then each call with its
        # outcome and each line the scenario printed. The derivation
        # succeeds when a scenario stops before `pass`: that is the
        # scenario's Wasmtime stage. `scenarios` holds each scenario's
        # `expectations.txt`, and `compiled` is `buildZenaScenarios`'s
        # output for the same directory. Nothing here is committed.
        runScenariosUnderWasmtime =
          {
            name,
            scenarios,
            compiled,
          }:
          pkgs.runCommand name { } ''
            ${wasmtimeRunner}/bin/wcmp-wasmtime ${scenarios} ${compiled} "$out"
          '';

        zenaWasmtime = runScenariosUnderWasmtime {
          name = "zena-wasmtime";
          scenarios = ./rust/wasm-component-model-polyfill/tests/zena/scenarios;
          compiled = zenaScenarios;
        };

        # Packs each scenario's expectations, compiled programs, and
        # Wasmtime observations into `$out/scenarios.bundle`, one file the
        # polyfill's `zena` test embeds at compile time: the browser lane
        # has no file system to read the build's directories from. The
        # script's header states the format.
        zenaBundler = pkgs.writeShellApplication {
          name = "zena-bundle-scenarios";
          runtimeInputs = [ pkgs.coreutils ];
          text = builtins.readFile ./rust/wasm-component-model-polyfill/tests/zena/bundle.sh;
        };

        zenaTestScenarios = pkgs.runCommand "zena-test-scenarios" { } ''
          mkdir -p "$out"
          ${pkgs.lib.getExe zenaBundler} \
            ${./rust/wasm-component-model-polyfill/tests/zena/scenarios} \
            ${zenaScenarios} ${zenaWasmtime} "$out/scenarios.bundle"
        '';

        # A build that compiles the polyfill's tests: the test archives
        # and the clippy check. The `zena` test runs each scenario through
        # the polyfill, in the browser and natively, and reads them from
        # the bundle this variable names.
        withZenaScenarios =
          derivation:
          derivation.overrideAttrs {
            WCMP_ZENA_SCENARIOS = "${zenaTestScenarios}/scenarios.bundle";
          };

        # The Wasmtime run against its own cases, compiled with the pinned
        # toolchain like any scenario, failing the check when one does not
        # hold:
        #
        # - A program that prints two lines passes only when both are
        #   captured in memory and match the expected lines, and the
        #   observations keep them.
        # - The same program against other expected lines, a wrong
        #   result, and a call to a missing export each stop before `pass`.
        # - A program Zena refuses stops at `compile` and makes no call.
        zenaWasmtimeCheck =
          let
            cases = ./rust/wasm-component-model-polyfill/tests/zena/wasmtime-check;
            observed = runScenariosUnderWasmtime {
              name = "zena-wasmtime-cases";
              scenarios = cases;
              compiled = buildZenaScenarios {
                name = "zena-wasmtime-case-programs";
                scenarios = cases;
              };
            };
          in
          pkgs.runCommand "zena-wasmtime-check" { } ''
            stage() { head -n 1 ${observed}/$1/observations.txt; }

            test "$(stage prints)" = "stage pass"
            test "$(grep '^output ' ${observed}/prints/observations.txt)" = \
              "$(printf 'output "hello"\noutput "world"')"

            test "$(stage prints-other-lines)" = \
              'stage mismatch "output line 2 is \"world\" where \"there\" was expected"'
            test "$(stage returns-other-result)" = \
              'stage mismatch "call 1 `scalar add(1s32, 2s32)` returned 3s32 where 4s32 was expected"'
            stage calls-a-missing-export | grep -q '^stage call "call 1 .*has no function export subtract"$'

            stage refused | grep -q '^stage compile "program refused did not compile (exit 1): '
            test "$(grep -c '^call ' ${observed}/refused/observations.txt)" = 0

            touch "$out"
          '';

        # The one Wasmtime of the workspace: `Cargo.lock` resolves
        # `wasmtime` to a single version, the one `Cargo.toml` pins, and
        # `wasmtime-wasi` to that same version, so the Wasmtime run and the
        # polyfill's native backend link the same Wasmtime.
        wasmtimeVersionCheck =
          let
            lock = builtins.fromTOML (builtins.readFile ./Cargo.lock);
            manifest = builtins.fromTOML (builtins.readFile ./Cargo.toml);
            pinned = pkgs.lib.removePrefix "=" manifest.workspace.dependencies.wasmtime;
            versions =
              name: map (package: package.version) (builtins.filter (package: package.name == name) lock.package);
            wasmtime = versions "wasmtime";
            wasi = versions "wasmtime-wasi";
          in
          pkgs.runCommand "wasmtime-version-check" { } (
            if wasmtime == [ pinned ] && wasi == [ pinned ] then
              ''touch "$out"''
            else
              ''
                echo "Cargo.lock resolves wasmtime to [${toString wasmtime}] and wasmtime-wasi to [${toString wasi}]; the workspace pins ${pinned} for both" >&2
                exit 1
              ''
          );

        # The wasm32 test runner (see `.cargo/config.toml`). nextest runs
        # each browser test in its own runner process, and the stock
        # `wasm-bindgen-test-runner` boots a ChromeDriver and a headless
        # Chrome and regenerates the wasm-bindgen glue for the whole test
        # binary on every one of those: about five seconds and two
        # gigabytes per test here, before the test body runs. wbg-pool keeps
        # one headless Chrome alive in a daemon, opens a fresh tab on a
        # fresh origin per test, and generates the glue once per binary.
        # Built from the pinned dialog-db tree; `cargoHash` covers that
        # workspace's lockfile.
        wbg-pool = pkgs.rustPlatform.buildRustPackage {
          pname = "wbg-pool";
          version = "0.1.0";
          src = wbg-pool-src;
          cargoHash = "sha256-W7rALlEbY3ZobZtNzI9xljEC0pkeMm0OLYbl2uvV9xU=";
          cargoBuildFlags = [
            "--package"
            "wbg-pool"
          ];
          # The crate's tests open sockets and spawn processes, which the
          # build sandbox forbids.
          doCheck = false;
          meta.mainProgram = "wbg-pool";
        };

        # Chrome differs by platform: Darwin uses google-chrome (unfree)
        # because chromium is unmaintained there; everything else uses
        # chromium.
        chrome = if pkgs.stdenv.isDarwin then pkgs.google-chrome else pkgs.chromium;
        chromePath = "${chrome}/bin/${chrome.meta.mainProgram}";

        # Headless Chrome refuses to start under the Nix build sandbox on
        # Linux and under the default sandbox/GPU configuration on Darwin.
        # wasm-bindgen-test-runner reads this JSON and passes the flags
        # through ChromeDriver on every platform. The browser lanes run
        # through wbg-pool instead (see `.cargo/config.toml`), which
        # launches Chrome itself; the stock runner remains its fallback for
        # a test binary not configured for a browser.
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
        # Scoped to the top-level README, CLAUDE.md, the design corpus, the
        # board README, and the benchmark README; the board file itself is
        # machine-managed and stays out of the gate.
        markdown = katsuobushi.lib.markdown {
          inherit pkgs;
          workspaceRoot = ./.;
          include = [
            "CLAUDE.md"
            "README.md"
            "project/design/**/*.md"
            "project/kanban/README.md"
            "rust/wcmp-bench/README.md"
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
          # file and knows the fixed-output hashes for the version it was
          # validated against, which is the one Cargo.toml pins. When the
          # workspace bumps `wasm-bindgen` past that, add a
          # `wasmBindgenHashes."<version>"` entry here (bootstrap its `hash`
          # and `cargoHash` with `pkgs.lib.fakeHash` and let the failing
          # build report the real values).
        };

        inherit (rustHelpers)
          buildCrate
          buildTestArchive
          buildTrunkCrate
          buildWasmCrate
          cargoChecks
          checkArtifactAlignment
          rustEnvironmentHook
          rustToolchain
          wasm-bindgen-cli
          ;

        # The public-API snapshot check.
        #
        # `cargo public-api` lists a crate's public surface by reading
        # rustdoc's JSON output, which no stable rustdoc emits: the switch
        # that turns it on is a `-Z` flag. The usual way around that —
        # setting `RUSTC_BOOTSTRAP` — turns every nightly feature on for the
        # whole build, so this flake takes the other route and pins an actual
        # nightly rustc, scoped to this one derivation. It comes from the
        # same rust-overlay the stable toolchain comes from, at a date fixed
        # here; `rust-toolchain.toml` stays on stable and nothing else in the
        # flake sees the nightly.
        publicApiToolchain =
          (pkgs.extend (import katsuobushi.inputs.rust-overlay)).rust-bin.nightly."2026-06-22".minimal;

        publicApiCrane =
          (katsuobushi.inputs.crane.mkLib pkgs).overrideToolchain
            (_: publicApiToolchain);

        # The snapshot's own source filter. `Cargo.lock` pins what the
        # rustdoc run compiles, and `rust/` carries both the crates and the
        # checked-in snapshot the build phase diffs against.
        publicApiSource = katsuobushi.inputs.nix-filter.lib {
          root = ./.;
          include = [
            "Cargo.lock"
            "Cargo.toml"
            "rust"
          ];
        };

        publicApiArguments = {
          src = publicApiSource;
          pname = "wasm-component-model-polyfill-public-api";
          version = "0.1.0";
          strictDeps = true;
          nativeBuildInputs = commonBuildInputs;
          doCheck = false;
        };

        # The public API of the polyfill crate, as the nightly rustdoc sees
        # it. `--simplified` three times drops the blanket, auto-trait, and
        # derived impls, which are noise every type carries and not a
        # decision anyone makes; what is left is the surface this workspace
        # chose to publish.
        publicApiListing = publicApiCrane.mkCargoDerivation (
          publicApiArguments
          // {
            cargoArtifacts = publicApiCrane.buildDepsOnly publicApiArguments;
            nativeBuildInputs = commonBuildInputs ++ [ pkgs.cargo-public-api ];
            buildPhaseCargoCommand = ''
              cargo public-api --package wasm-component-model-polyfill -sss \
                --color never > public-api.txt
            '';
            installPhaseCommand = ''
              mkdir -p "$out"
              cp public-api.txt "$out/public-api.txt"
            '';
            doNotPostBuildInstallCargoBinaries = true;
          }
        );

        # The listing against the one checked in beside the crate. A `pub`
        # that reaches a workspace-internal type — a field, a constructor, a
        # method whose type no caller outside can name — is a line in the
        # listing, so it arrives as a diff in review rather than as a piece
        # of surface nobody noticed was nameable.
        publicApiCheck =
          pkgs.runCommand "wasm-component-model-polyfill-public-api-check" { }
            ''
              if ! diff -u ${./rust/wasm-component-model-polyfill/public-api.txt} \
                ${publicApiListing}/public-api.txt; then
                echo >&2
                echo "The public API changed. If every line above is intended," >&2
                echo "record the new surface with:" >&2
                echo >&2
                echo "  api update" >&2
                echo >&2
                exit 1
              fi
              touch "$out"
            '';

        developmentBuildInputs =
          commonBuildInputs
          ++ (with pkgs; [
            cargo-nextest
            chrome
            chromedriver
            rustToolchain
            wasm-bindgen-cli
            wasm-tools
          ])
          ++ [
            markdown.prettier
            # The wasm32 test runner nextest invokes once per browser test
            # against one shared headless Chrome.
            wbg-pool
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
          # The pooled browser gets the same `--no-sandbox` the WebDriver
          # configuration passes to the stock runner: Chrome's sandbox does
          # not initialize inside a sandbox VM, and test code is trusted.
          "WBG_POOL_NO_SANDBOX" = "1";
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
            -E 'test(=it_reports_conformance_progress)'
          rm -rf "''${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/${package}-summary"
        '';
        conformanceSummaryCommand = conformanceSummaryFor "tests-native-debug";

        # `tests regenerate`: rewrites `tests/corpus/expected-failures.txt`
        # from a native run, in place of the hand loop that blanked the
        # list, reran the suite, and merged the printed `unexpected:` lines
        # back by hand — a loop that once dropped 28 hand-written
        # parentheticals from lines nothing had touched. The harness does
        # the merge (`WCMP_REGENERATE_EXPECTATIONS` names the list it
        # rewrites): a directive that still fails keeps its category and
        # its parenthetical and takes the run's reason, a directive that
        # passes loses its line, and a new failure arrives with a
        # placeholder category the harness rejects until a person replaces
        # it. A second run, with the suspend provider turned off, then
        # rewrites `expected-failures.no-provider.txt` the same way from
        # the failures the regenerated shared list does not name
        # (`WCMP_REGENERATE_BASE` names that list). The runs always write
        # copies under the lane workspace and the diffs are printed;
        # `--dry-run` stops there, which is also how to read the live
        # reason of a directive a list already names.
        regenerateExpectationsCommand = ''
          corpus="$(git rev-parse --show-toplevel)"/rust/wasm-component-model-polyfill/tests/corpus
          dry=""
          for argument in "$@"; do
            case "$argument" in
              --dry-run) dry=1 ;;
              *)
                echo "tests regenerate: $argument is not an argument of this command (only --dry-run)" >&2
                exit 2
                ;;
            esac
          done
          archive=$(nix build --no-link --print-out-paths .#tests-native-debug)
          workspace="''${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/tests-native-debug-regenerate"
          rm -rf "$workspace"
          mkdir -p "$workspace/archive"
          trap 'rm -rf "$workspace"' EXIT
          shared="$workspace/expected-failures.txt"
          overlay="$workspace/expected-failures.no-provider.txt"
          cp "$corpus/expected-failures.txt" "$shared"
          cp "$corpus/expected-failures.no-provider.txt" "$overlay"
          # One progress test per run: the overlay's run reads the shared
          # list the first run wrote, so the two cannot run side by side.
          regenerate() {
            cargo nextest run \
              --workspace-remap ./ \
              --archive-file "$archive/tests-native-debug.tar.zst" \
              --extract-to "$workspace/archive" \
              --extract-overwrite \
              --ignore-default-filter \
              --no-capture \
              -E "test(=$1)"
          }
          WCMP_REGENERATE_EXPECTATIONS="$shared" \
            regenerate it_reports_conformance_progress
          WCMP_REGENERATE_EXPECTATIONS="$overlay" WCMP_REGENERATE_BASE="$shared" \
            regenerate it_reports_conformance_progress_without_a_provider
          for name in expected-failures.txt expected-failures.no-provider.txt; do
            echo
            if diff -u "$corpus/$name" "$workspace/$name"; then
              echo "tests regenerate: $name is current, nothing to write"
            elif [ -n "$dry" ]; then
              echo "tests regenerate: the diff above is what a run would write; $name is unchanged"
            else
              install -m 644 "$workspace/$name" "$corpus/$name"
              echo "tests regenerate: wrote $corpus/$name"
            fi
          done
        '';

        # Every replay extracts the archive (4 to 7 GB of test binaries)
        # into a directory on disk under the user's cache directory, not
        # under `$TMPDIR`, which is a RAM-backed tmpfs on most Linux
        # systems, and removes it when nextest exits. The browsers a web
        # lane starts write their profiles under the same directory. The
        # path is kept short on purpose: Chromium puts Unix sockets under
        # `$TMPDIR`, and a socket path longer than 108 bytes aborts the
        # browser at startup.
        testWorkspace = lane: ''
          workspace="''${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/${lane}"
          rm -rf "$workspace"
          mkdir -p "$workspace/archive" "$workspace/tmp"
          trap 'rm -rf "$workspace"' EXIT
          export TMPDIR="$workspace/tmp"
        '';

        # A web lane runs every test in a tab of one shared headless Chrome,
        # so a test in flight costs one renderer holding an instantiated
        # debug test module, well under a gigabyte. Its parallelism follows
        # available memory, one test per gigabyte, capped at the core
        # count, unless the operator sets `NEXTEST_TEST_THREADS` or passes
        # `-j`. Native lanes keep nextest's default, one test per core.
        browserTestThreads = ''
          if [ -z "''${NEXTEST_TEST_THREADS:-}" ]; then
            threads=4
            if [ -r /proc/meminfo ]; then
              available_kb=$(awk '/MemAvailable/ { print $2 }' /proc/meminfo)
              threads=$(( available_kb / 1024 / 1024 ))
            fi
            cores=$(nproc 2>/dev/null || echo "$threads")
            [ "$threads" -gt "$cores" ] && threads=$cores
            [ "$threads" -lt 1 ] && threads=1
            export NEXTEST_TEST_THREADS="$threads"
            echo "browser tests: $threads at a time (set NEXTEST_TEST_THREADS or pass -j to change)"
          fi
        '';

        # The pooled browser's daemon is detached from the shim that spawns
        # it and would idle for five minutes on its own, so a browser lane
        # pins its rendezvous directory under the lane workspace and stops
        # it on exit, before the workspace (and the browser's files under
        # it) goes away. `--stop` returns before the daemon has finished
        # removing its own files, so a removal that races it can meet a
        # directory that is not empty yet; it retries for a few seconds,
        # and only the last attempt can fail the lane.
        browserPool = ''
          export WBG_POOL_DIR="$workspace/wbg-pool"
          trap 'wbg-pool daemon --stop >/dev/null 2>&1
            for _ in 1 2 3 4 5; do
              rm -rf "$workspace" 2>/dev/null && break
              sleep 1
            done
            rm -rf "$workspace"' EXIT
        '';

        menuTestCommand =
          {
            description,
            package,
            # The nextest profile (`.config/nextest.toml`) that picks the
            # lane's tests from the archive.
            nextestProfile ? "default",
            # Print the conformance progress summary after the run.
            summary ? false,
            # Cap the parallelism from available memory (web lanes).
            browser ? false,
          }:
          let
            # Lanes that replay one archive under different profiles get
            # workspaces of their own.
            lane = if nextestProfile == "default" then package else "${package}-${nextestProfile}";
          in
          {
            inherit description;
            command = ''
              archive=$(nix build --no-link --print-out-paths .#${package})
            ''
            + testWorkspace lane
            + pkgs.lib.optionalString browser (browserPool + browserTestThreads)
            + ''
              cargo nextest run \
                --profile ${nextestProfile} \
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

          # The benchmark suite. Nix builds the suite; the measurement
          # itself runs here rather than inside a derivation, because a
          # derivation's output is cached and a cached benchmark result
          # is a stale one. Both lanes write their JSON report under the
          # cargo target directory, as the conformance summary does, and
          # both take `key=value` run controls after the leaf, for
          # example `bench native samples=50 warmup=4`.
          "bench" = {
            description = "Measure the polyfill on one target with the benchmark suite";
            subcommands = {
              native = {
                description = "Run the benchmark suite on ${system}";
                command = ''
                  report="''${CARGO_TARGET_DIR:-target}/bench/native.json"
                  mkdir -p "$(dirname "$report")"
                  binary=$(nix build --no-link --print-out-paths .#bench-native)
                  WCMP_BENCH_REPORT="$report" "$binary"/bin/wcmp-bench "$@"
                '';
              };
              web = {
                description = "Run the same suite in headless Chrome, timed with performance.now()";
                command = ''
                  report="''${CARGO_TARGET_DIR:-target}/bench/web.json"
                  mkdir -p "$(dirname "$report")"
                  site=$(nix build --no-link --print-out-paths .#bench-web)
                  ${pkgs.python3}/bin/python3 ${./rust/wcmp-bench/web/run.py} \
                    "$site" "$report" "$@"
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
                  no-provider = menuTestCommand {
                    description = "The conformance corpus alone, with the suspend provider turned off through `EngineConfig` (${system}, debug)";
                    package = "tests-native-debug";
                    nextestProfile = "no-provider";
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
                  no-provider = menuTestCommand {
                    description = "The conformance corpus alone, with the suspend provider turned off through `EngineConfig` (wasm32-unknown-unknown, debug)";
                    package = "tests-web-debug";
                    nextestProfile = "no-provider";
                    browser = true;
                  };
                };
              };
              conformance = {
                description = "Conformance progress per corpus on both targets (debug)";
                command = conformanceSummaryCommand + conformanceSummaryFor "tests-web-debug";
              };
              regenerate = {
                description = "Rewrite the shared expected-failure list from the native run with the suspend provider and the no-provider overlay from the native run without it (`--dry-run` only prints the diffs)";
                command = regenerateExpectationsCommand;
              };
              # The end-to-end smoke test (`rust/wcmp-smoke`): one host
              # program that tells what a developer does with the polyfill,
              # story by story, natively or in a browser.
              smoke = {
                description = "The end-to-end smoke test, natively or in a browser";
                subcommands = {
                  native = {
                    description = "Build the smoke test host as a derivation and run it";
                    command = ''
                      "$(nix build --no-link --print-out-paths .#smoke-native)"/bin/wcmp-smoke
                    '';
                  };
                  web = {
                    description = "Build the smoke test page and serve it (`nix run .#smoke-web`; a port after the leaf, 8765 by default)";
                    command = ''
                      nix run .#smoke-web -- "$@"
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
              # The conformance corpus runs in four states: each target with
              # the suspend provider allowed (the debug and release lanes)
              # and with it turned off (the `no-provider` lanes). Each lane
              # reports its wall-clock time, build included.
              all = {
                description = "Every lane, each timed: both targets in debug and release, and the conformance corpus on both targets with the suspend provider off, so the corpus runs in all four states (grab a coffee)";
                command = ''
                  status=0
                  for suite in "native debug" "native release" "native no-provider" \
                    "web debug" "web release" "web no-provider"; do
                    started=$SECONDS
                    # shellcheck disable=SC2086
                    if tests $suite "$@"; then
                      echo "tests $suite: passed in $((SECONDS - started))s"
                    else
                      echo "tests $suite: FAILED after $((SECONDS - started))s"
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

          # The crate's public surface, as `cargo public-api` reads it out
          # of nightly rustdoc's JSON. `list` prints it; `update` records it
          # as the snapshot the `public-api` flake check diffs against.
          "api" = {
            description = "Print or record the polyfill crate's public surface";
            subcommands = {
              list = {
                description = "Print the public surface the crate exposes today";
                command = ''
                  cat "$(nix build --no-link --print-out-paths .#public-api)"/public-api.txt
                '';
              };
              update = {
                description = "Record today's public surface as the checked-in snapshot";
                command = ''
                  listing=$(nix build --no-link --print-out-paths .#public-api)
                  snapshot="$(git rev-parse --show-toplevel)"/rust/wasm-component-model-polyfill/public-api.txt
                  install -m 644 "$listing"/public-api.txt "$snapshot"
                  echo "recorded $(wc -l < "$snapshot") public items in $snapshot"
                '';
              };
            };
          };

          # The real-guest fixtures under the conformance corpus, rebuilt
          # from their WIT, WAT, and Rust sources with the flake's pinned
          # `wasm-tools`, `wac`, and Rust toolchain, so a rerun on the same
          # system reproduces every output byte for byte until one of the
          # tools is bumped on purpose. The fixtures README records how far
          # that claim reaches beyond the machine it was observed on.
          "fixtures" = {
            description = "Regenerate the conformance fixtures with cargo, wasm-tools, and wac";
            command = ''
              export PATH=${pkgs.wasm-tools}/bin:${wac-cli}/bin:${rustToolchain}/bin:$PATH
              "$(git rev-parse --show-toplevel)"/rust/wasm-component-model-polyfill/tests/corpus/fixtures/build.sh
            '';
          };

        }
        // markdown.menuCommands
        // project.menuCommands
        # `sandbox start` / `prompt` / `status` / `attach` / `fetch` /
        # `deliver` / `stop` / `dispatch` / `prune`. Every `<inst>` also
        # accepts the index shown in `sandbox status`.
        // pkgs.lib.optionalAttrs isLinux sandbox.menuCommands;

        # The smoke test host (`rust/wcmp-smoke`): one program that tells
        # what a developer does with the polyfill, story by story. Natively
        # it is a binary; for the browser, Trunk builds the same binary
        # crate into a page from the `Trunk.toml` beside the crate and the
        # `web/index.html` it names, with the page's stylesheet and script.
        smokeNative = buildCrate {
          pname = "wcmp-smoke";
          version = "0.1.0";
          cargoExtraArgs = "--package wcmp-smoke";
        };
        smokeWeb = buildTrunkCrate {
          pname = "wcmp-smoke-web";
          version = "0.1.0";
          trunkConfig = "rust/wcmp-smoke/Trunk.toml";
          trunkIndexPath = "web/index.html";
          # Trunk writes `dist` beside `Trunk.toml`, not beside the page.
          installPhaseCommand = ''
            cp -r dist $out
          '';
        };

        # The page served for a person to open: `nix run .#smoke-web`, which
        # `tests smoke web` runs. static-web-server serves the built page on
        # a loopback port, and every response says `no-store`: the page's
        # files come out of the Nix store with a 1970 modification time,
        # and a browser given that as `Last-Modified` would keep the script
        # and the wasm across runs, so a rebuilt page would show the
        # previous build until a hard reload.
        smokeWebServerConfig = (pkgs.formats.toml { }).generate "static-web-server.toml" {
          advanced.headers = [
            {
              source = "**/*";
              headers."Cache-Control" = "no-store";
            }
          ];
        };
        smokeWebServer = pkgs.writeShellApplication {
          name = "wcmp-smoke-web";
          runtimeInputs = [ pkgs.static-web-server ];
          text = ''
            port="''${1:-8765}"
            echo "smoke test page: http://127.0.0.1:$port/  (Ctrl-C stops the server)"
            exec static-web-server \
              --config-file ${smokeWebServerConfig} \
              --root ${smokeWeb} \
              --host 127.0.0.1 \
              --port "$port" \
              --cache-control-headers=false \
              --log-level warn
          '';
        };

        # The web smoke page driven headlessly, as a check: the page
        # `tests smoke web` serves, loaded in the flake's Chromium through
        # chromedriver inside the build sandbox by `web/check.sh`, with its
        # report compared against the native smoke binary's. The script
        # speaks WebDriver over HTTP with `curl` and `jq`, and serves the
        # page with the same static-web-server a person is served by.
        smokeWebCheckDriver = pkgs.writeShellApplication {
          name = "wcmp-smoke-web-check";
          runtimeInputs = [
            pkgs.coreutils
            pkgs.curl
            pkgs.jq
            pkgs.chromedriver
            pkgs.static-web-server
          ];
          text = builtins.readFile ./rust/wcmp-smoke/web/check.sh;
        };
        smokeWebCheck =
          pkgs.runCommand "wcmp-smoke-web-check"
            {
              nativeBuildInputs = [
                chrome
                smokeWebCheckDriver
              ];
              WASM_BINDGEN_TEST_WEBDRIVER_JSON = webdriverConfig;
            }
            ''
              export HOME=$TMPDIR
              native=$(${smokeNative}/bin/wcmp-smoke | tail -n 1)
              mkdir -p $out
              wcmp-smoke-web-check ${smokeWeb} "$native" $out/report.txt
            '';

        # The benchmark suite (`rust/wcmp-bench`): one binary that
        # measures the polyfill, natively and, through the same
        # `wasm-bindgen` path the smoke page takes, in a browser. The
        # `bench` menu command builds these and then runs them; a
        # measurement is never a derivation's output, because that
        # output would be cached and a cached benchmark is stale.
        benchNative = buildCrate {
          pname = "wcmp-bench";
          version = "0.1.0";
          cargoExtraArgs = "--package wcmp-bench";
        };
        benchWeb = buildWasmCrate {
          pname = "wcmp-bench-web";
          version = "0.1.0";
          cargoExtraArgs = "--package wcmp-bench --bin wcmp-bench";
          doInstallCargoArtifacts = false;
          doNotPostBuildInstallCargoBinaries = true;
          installPhaseCommand = ''
            mkdir -p $out
            $WASM_BINDGEN_BIN --target web --no-typescript \
              --out-dir $out target/wasm32-unknown-unknown/release/wcmp-bench.wasm
            cp ${./rust/wcmp-bench/web/index.html} $out/index.html
          '';
        };

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
        testsNativeDebug = withZenaScenarios (buildTestArchive {
          name = "native-debug";
          profile = "dev";
        });

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
        apps = {
          # The smoke test page on a loopback port: `nix run .#smoke-web`
          # (`tests smoke web` inside the shell), with an optional port
          # after `--`.
          smoke-web = {
            type = "app";
            program = "${smokeWebServer}/bin/wcmp-smoke-web";
            meta.description = "Serve the smoke test page on a loopback port";
          };
        }
        # `nix run .#sandbox -- --agent --name <name>` is `sandbox start`
        # from outside the dev shell.
        // pkgs.lib.optionalAttrs isLinux {
          sandbox = sandbox.apps.sandbox;
        };

        packages = {
          # The component toolchain the `fixtures` command runs, exposed so
          # the pinned versions are one `nix build` away.
          wasm-tools = pkgs.wasm-tools;
          wac = wac-cli;
          # The browser test runner, exposed so its build is one `nix build`
          # away when its pin or the `wasm-bindgen` version moves.
          inherit wbg-pool;
          # The pinned Zena toolchain, and every Zena scenario compiled with
          # it. Neither enters the development shell.
          zena = zenaToolchain;
          zena-scenarios = zenaScenarios;
          # Every Zena scenario run through Wasmtime: each scenario's
          # observations and Wasmtime stage, which the polyfill subjects
          # read.
          zena-wasmtime = zenaWasmtime;

          smoke-native = smokeNative;
          smoke-web = smokeWeb;

          bench-native = benchNative;
          bench-web = benchWeb;
          public-api = publicApiListing;

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

          tests-native-release = withZenaScenarios (buildTestArchive {
            name = "native-release";
          });

          tests-web-debug = withZenaScenarios (buildTestArchive {
            name = "web-debug";
            target = "wasm32-unknown-unknown";
            profile = "dev";
          });

          tests-web-release = withZenaScenarios (buildTestArchive {
            name = "web-release";
            target = "wasm32-unknown-unknown";
          });

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
            # Clippy compiles every target, the `zena` test included, and
            # that test embeds the Zena scenarios at compile time.
            clippy = withZenaScenarios cargoChecks.clippy;
            # The web smoke page must still run, and report what the native
            # binary reports: see `smokeWebCheck`.
            smoke-web = smokeWebCheck;
            # The suite must build for both targets on every check; running
            # it stays a menu command, so a number is never a cached one.
            bench-native = benchNative;
            bench-web = benchWeb;
            # The crate's public surface must still be the checked-in one:
            # see `publicApiCheck`.
            public-api = publicApiCheck;
            # Every Zena scenario compiles, or keeps Zena's refusal, with the
            # pinned toolchain; and the scenario build keeps a refusal as
            # its output: see `buildZenaScenarios`.
            zena-scenarios = zenaScenarios;
            zena-scenario-build = zenaScenarioBuildCheck;
            # Every Zena scenario runs through Wasmtime and writes its
            # observations, whatever its stage; the Wasmtime run holds
            # against its own cases; and it links the workspace's one
            # Wasmtime. See `runScenariosUnderWasmtime`.
            zena-wasmtime = zenaWasmtime;
            zena-wasmtime-run = zenaWasmtimeCheck;
            wasmtime-version = wasmtimeVersionCheck;
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
          }
          // pkgs.lib.optionalAttrs isLinux {
            # Builds the guest image, so a broken sandbox configuration fails
            # here rather than at launch; and confirms the `sandbox` wrapper
            # and `katsuctl` agree on the verb set.
            sandbox = sandbox.checks.sandbox;
            sandbox-verb-coverage = sandbox.checks.sandbox-verb-coverage;
          };

        devShells.default = pkgs.mkShell {
          name = "wcmp";
          env = developmentEnvVars;
          nativeBuildInputs =
            menu.commands
            ++ developmentBuildInputs
            # The `sandbox` commands invoke `katsuctl` by its store path; a
            # bare `katsuctl` on the PATH is for driving the controller
            # directly.
            ++ pkgs.lib.optionals isLinux [ sandbox.katsuctl ];
          shellHook = rustEnvironmentHook + makeDevShellHook menu;
        };
      }
    );
}
