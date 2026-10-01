{
  description = "Wasm Component Model Polyfill";

  # Katsuobushi carries the Rust build infra (crane, nix-filter, rust-overlay)
  # and the sandbox infra (microvm.nix) as transitive inputs, so this flake
  # declares only nixpkgs, flake-utils, and katsuobushi, plus the project-data
  # sources the sandbox guest carries. `nixpkgs.follows` keeps katsuobushi and
  # everything it builds on this flake's nixpkgs.
  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    # The systems list `eachDefaultSystem` reads (see `nix/systems.nix`).
    # Katsuobushi's flake-utils follows this flake's, so that katsuobushi,
    # whose nixpkgs is this one, is never evaluated for a system it dropped.
    systems = {
      url = "path:./nix/systems.nix";
      flake = false;
    };
    flake-utils.url = "github:numtide/flake-utils";
    flake-utils.inputs.systems.follows = "systems";
    katsuobushi.url = "github:cdata/katsuobushi/v0.5.1";
    katsuobushi.inputs.nixpkgs.follows = "nixpkgs";
    katsuobushi.inputs.flake-utils.follows = "flake-utils";

    # The agent harness that runs inside a sandbox VM. Pre-built upstream, so
    # it needs no unfree allowance, and newer than the nixpkgs build. It keeps
    # its own nixpkgs: its packages track a newer nixpkgs than this flake
    # pins, and only the guest's harness closure is built from it.
    llm-agents.url = "github:numtide/llm-agents.nix";

    # Project-data sources for the sandbox guest. `flake = false` fetches the
    # tree only; `nix flake update` moves the pins. The two reference repos
    # are the ones a design review cites side by side with this crate.
    component-model-src = {
      url = "github:WebAssembly/component-model";
      flake = false;
    };
    wasmtime-src = {
      url = "github:bytecodealliance/wasmtime";
      flake = false;
    };

    # The official WebAssembly specification test suite, the fidelity
    # suite of the runtime layer, pinned by commit in the URL so that `nix
    # flake update` cannot move it. The commit is the one Wasmtime
    # 49.0.0-rc.1, the control's engine, pins as its own `spec_testsuite`
    # submodule. Moving the pin is a deliberate change: every backend's
    # expected failures are held against it.
    spec-testsuite = {
      url = "github:WebAssembly/testsuite/0dc0343c9876267d99a7577ed4fc2289406a7869";
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

  # The project's binary cache. CI pushes to it, so a shell or a build that
  # CI has made comes down instead of rebuilding. Nix asks before it uses a
  # flake's settings; `--accept-flake-config` skips the question.
  nixConfig = {
    extra-substituters = [
      "https://wcmp.cachix.org"
    ];
    extra-trusted-public-keys = [
      "wcmp.cachix.org-1:5qVYDJO8yXbHEM2CKOv9/XeE1asTVay7FmxFpAmxAt0="
    ];
  };

  outputs =
    {
      self,
      systems,
      nixpkgs,
      flake-utils,
      katsuobushi,
      llm-agents,
      component-model-src,
      wasmtime-src,
      spec-testsuite,
      wbg-pool-src,
      zena,
    }:
    flake-utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          # The Rust helper applies rust-overlay internally, so only the
          # katsuobushi overlay (menu helpers) and the Trunk fix below are
          # needed here.
          overlays = [
            katsuobushi.overlays.default
            # Trunk, which builds the smoke test page, vendors libdeflate
            # 1.23, whose x86 code names the `evex512` target attribute that
            # GCC 16 removed; the nixpkgs build fails on x86_64 Linux and
            # cache.nixos.org has no copy. libdeflate names the attribute
            # only while `__EVEX512__` is undefined, so defining it for the
            # C that cc-rs compiles leaves the attribute out. (A GCC 15
            # stdenv does not help: the cargo hook hands cc-rs the default
            # compiler.) Remove this once nixpkgs' Trunk builds again.
            (final: prev: {
              trunk =
                if prev.stdenv.hostPlatform.system == "x86_64-linux" then
                  prev.trunk.overrideAttrs (old: {
                    env = (old.env or { }) // {
                      CFLAGS_x86_64_unknown_linux_gnu = "-D__EVEX512__";
                    };
                  })
                else
                  prev.trunk;
            })
          ];
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
        isLinux = pkgs.stdenv.hostPlatform.isLinux;

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
          # server (a toolchain the host has not built yet), docs.rs (the
          # preferred place to read a dependency's API), and the project's
          # binary cache, which serves its NARs from its own host.
          allowedOrigins = [
            "static.rust-lang.org"
            "crates.io"
            "index.crates.io"
            "static.crates.io"
            "docs.rs"
            "wcmp.cachix.org"
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

          # Untracked, project-local Claude Code configuration (a
          # `.claude/settings.json` pins the in-guest model, for example).
          # The owner's universal agent rules travel here too, as an
          # untracked `.claude/CLAUDE.md`, rather than as a flake input: the
          # repository that holds them is private, and every input must be
          # fetchable by CI and by anyone who clones this one. Carried when
          # present; a missing path is skipped at launch. Symlinks that leave
          # the tree are dropped, so the file must be a copy.
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

              # The binary cache `nixConfig` names, for what the host has not
              # built and CI has. The guest's menu commands pass no
              # `--accept-flake-config`, so the daemon is told directly. The
              # `extra-` forms add to the cache.nixos.org entries rather than
              # replace them.
              nix.settings.extra-substituters = [ "https://wcmp.cachix.org" ];
              nix.settings.extra-trusted-public-keys = [
                "wcmp.cachix.org-1:5qVYDJO8yXbHEM2CKOv9/XeE1asTVay7FmxFpAmxAt0="
              ];

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
          text = builtins.readFile ./rust/wcmp/tests/zena/build.sh;
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

        # The script that builds one Rust partner of a scenario. Its header
        # states the layout of a partner and of the output. It runs with
        # this workspace's Rust toolchain and nothing from Zena's.
        zenaPartnerBuilder = pkgs.writeShellApplication {
          name = "zena-build-partner";
          runtimeInputs = [
            pkgs.coreutils
            pkgs.wasm-tools
            rustToolchain
          ];
          text = builtins.readFile ./rust/wcmp/tests/zena/partner.sh;
        };

        partnerCrane = katsuobushi.inputs.crane.mkLib pkgs;

        # The Rust partners of a directory of scenarios, found at
        # evaluation time: every directory `<scenario>/<partner>/` that
        # holds a `cargo-manifest.toml`. Each is `{ scenario, partner,
        # component }`, where `component` builds it from its locked
        # manifest with its crates vendored from its lock. The build's
        # source is the partner's directory and the scenario's `wit/`, and
        # nothing in it comes from the `zena` input, so a move of the pin
        # leaves every partner's derivation, and its bytes, as they were.
        rustPartners =
          scenarios:
          let
            inherit (pkgs.lib) attrNames filterAttrs concatMap;
            directories = path: attrNames (filterAttrs (_: type: type == "directory") (builtins.readDir path));
          in
          concatMap (
            scenario:
            map
              (partner: {
                inherit scenario partner;
                component =
                  let
                    root = scenarios + "/${scenario}";
                    source = pkgs.lib.fileset.toSource {
                      inherit root;
                      fileset = pkgs.lib.fileset.unions [
                        (root + "/wit")
                        (root + "/${partner}")
                      ];
                    };
                    vendor = partnerCrane.vendorCargoDeps {
                      cargoLock = root + "/${partner}/cargo-lock.toml";
                    };
                  in
                  pkgs.runCommandCC "zena-partner-${scenario}-${partner}" { } ''
                    ${pkgs.lib.getExe zenaPartnerBuilder} ${source} ${partner} \
                      ${vendor}/config.toml "$out"
                  '';
              })
              (
                builtins.filter (
                  partner: builtins.pathExists (scenarios + "/${scenario}/${partner}/cargo-manifest.toml")
                ) (directories (scenarios + "/${scenario}"))
              )
          ) (directories scenarios);

        # The script that composes the components of each scenario whose
        # wiring asks for a composition, with the `wac` the conformance
        # fixtures use. Its header states the layout it leaves.
        zenaComposer = pkgs.writeShellApplication {
          name = "zena-compose-scenarios";
          runtimeInputs = [
            pkgs.coreutils
            wac-cli
          ];
          text = builtins.readFile ./rust/wcmp/tests/zena/compose.sh;
        };

        # Every scenario under `scenarios` compiled: the Zena programs with
        # the pinned toolchain (`buildZenaScenarios`) and the Rust partners
        # with this workspace's (`rustPartners`), side by side under
        # `$out/<scenario>/`, so the steps after this one read a partner
        # as they read a program. A partner named like a program of its
        # scenario fails the build. Then `zenaComposer` composes each
        # scenario whose wiring asks for it into one component under the
        # importer's name, or keeps `wac`'s refusal as the scenario's
        # outcome.
        compileScenarios =
          { name, scenarios }:
          let
            programs = buildZenaScenarios {
              name = "${name}-programs";
              inherit scenarios;
            };
          in
          pkgs.runCommand name { passthru = { inherit zenaRevision; }; } (
            ''
              cp -R ${programs} "$out"
              chmod -R u+w "$out"
            ''
            + pkgs.lib.concatMapStrings (
              { scenario, partner, component }:
              ''
                if [ -e "$out/${scenario}/${partner}.status" ]; then
                  echo "zena: scenario ${scenario} has a program and a partner named ${partner}" >&2
                  exit 1
                fi
                cp ${component}/* "$out/${scenario}/"
              ''
            ) (rustPartners scenarios)
            + ''
              chmod -R u+w "$out"
              ${pkgs.lib.getExe zenaComposer} ${scenarios} "$out"
            ''
          );

        zenaScenarios = compileScenarios {
          name = "zena-scenarios";
          scenarios = ./rust/wcmp/tests/zena/scenarios;
        };

        # The Rust partners of the scenarios alone, one directory per
        # scenario that has one, for a person to inspect.
        zenaPartners = pkgs.runCommand "zena-partners" { } (
          ''
            mkdir -p "$out"
          ''
          + pkgs.lib.concatMapStrings (
            { scenario, component, ... }:
            ''
              mkdir -p "$out/${scenario}"
              cp ${component}/* "$out/${scenario}/"
            ''
          ) (rustPartners ./rust/wcmp/tests/zena/scenarios)
        );

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
        # - A directory with no scenario, or a scenario with neither a
        #   program nor a Rust partner, fails the build instead of yielding
        #   an empty output. A scenario with partners only builds, and
        #   leaves its directory empty for the partners. These run the
        #   script alone, with a stand-in for Zena that never runs.
        # - A composition link that names a missing program, or programs
        #   that compose in a chain, fail the composer; a composition
        #   whose exporter did not compile is left as it is, for the
        #   `compile` stage. These run the composer alone, on layouts that
        #   never reach `wac`.
        zenaScenarioBuildCheck =
          let
            built = buildZenaScenarios {
              name = "zena-scenario-build-cases";
              scenarios = ./rust/wcmp/tests/zena/build-check;
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
            grep -q 'scenario empty holds no .zena program and no Rust partner' \
              no-program.err

            mkdir -p partners-only/baseline/partner
            touch partners-only/baseline/partner/cargo-manifest.toml
            ${pkgs.lib.getExe zenaScenarioBuilder} partners-only partners-only.out
            test -d partners-only.out/baseline
            test -z "$(ls partners-only.out/baseline)"

            # The composer on wirings it cannot make, which fail the
            # build, and on a composition whose exporter did not compile,
            # which it leaves as it is. None of them reaches `wac`.
            for layout in missing chain uncompiled; do
              mkdir -p "compose-$layout/sources/demo" "compose-$layout/built/demo"
            done
            echo 'composition importer local:demo/api exporter' \
              >compose-missing/sources/demo/wiring.txt
            echo 0 >compose-missing/built/demo/importer.status
            printf '%s\n' 'composition a local:demo/b b' 'composition b local:demo/c c' \
              >compose-chain/sources/demo/wiring.txt
            for program in a b c; do
              echo 0 >compose-chain/built/demo/$program.status
            done
            echo 'composition importer local:demo/api exporter' \
              >compose-uncompiled/sources/demo/wiring.txt
            echo 0 >compose-uncompiled/built/demo/importer.status
            echo 1 >compose-uncompiled/built/demo/exporter.status
            for layout in missing chain; do
              if ${pkgs.lib.getExe zenaComposer} compose-$layout/sources \
                compose-$layout/built 2>compose-$layout.err; then
                echo "the composer accepted the $layout wiring" >&2
                exit 1
              fi
            done
            grep -q 'scenario demo has no program exporter' compose-missing.err
            grep -q 'b is both an importer and an exporter of compositions' \
              compose-chain.err
            ${pkgs.lib.getExe zenaComposer} compose-uncompiled/sources \
              compose-uncompiled/built
            test -e compose-uncompiled/built/demo/exporter.status
            test ! -e compose-uncompiled/built/demo/importer.compose-status

            touch "$out"
          '';

        # The Wasmtime run of the scenarios (`rust/wcmp-scenario-wasmtime`): a
        # native program built against this workspace's Wasmtime, the one
        # the polyfill's native backend links, with `wasmtime-wasi` at the
        # same version. Zena's own flake pins another Wasmtime, which only
        # the compile step above runs, inside Zena's command; it never
        # runs a scenario.
        wasmtimeRunner = buildCrate {
          pname = "wcmp-scenario-wasmtime";
          version = "0.1.0";
          cargoExtraArgs = "--package wcmp-scenario-wasmtime";
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
            ${wasmtimeRunner}/bin/wcmp-scenario-wasmtime ${scenarios} ${compiled} "$out"
          '';

        zenaWasmtime = runScenariosUnderWasmtime {
          name = "zena-wasmtime";
          scenarios = ./rust/wcmp/tests/zena/scenarios;
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
          text = builtins.readFile ./rust/wcmp/tests/zena/bundle.sh;
        };

        bundleZenaScenarios =
          {
            name,
            scenarios,
            compiled,
            observed,
          }:
          pkgs.runCommand name { } ''
            mkdir -p "$out"
            ${pkgs.lib.getExe zenaBundler} \
              ${scenarios} ${compiled} ${observed} "$out/scenarios.bundle"
          '';

        zenaTestScenarios = bundleZenaScenarios {
          name = "zena-test-scenarios";
          scenarios = ./rust/wcmp/tests/zena/scenarios;
          compiled = zenaScenarios;
          observed = zenaWasmtime;
        };

        # Two scenarios that stop before any component runs, taken through
        # every step a real scenario takes: compiled with the pinned
        # toolchain, composed, run through Wasmtime, and bundled. In
        # `refused`, Zena refuses the program; in `unplugged`, `wac`
        # refuses the composition. The `zena` test checks that each
        # subject stops them at `compile` and `compose`, and that the
        # record written from the run keeps the tool's error output.
        # Neither is in the record.
        zenaRecordCheck =
          let
            scenarios = ./rust/wcmp/tests/zena/record-check;
            compiled = compileScenarios {
              name = "zena-record-check-scenarios";
              inherit scenarios;
            };
          in
          bundleZenaScenarios {
            name = "zena-record-check";
            inherit scenarios compiled;
            observed = runScenariosUnderWasmtime {
              name = "zena-record-check-wasmtime";
              inherit scenarios compiled;
            };
          };

        # A build that compiles the polyfill's tests: the test archives
        # and the clippy check. The `zena` test runs each scenario through
        # the polyfill, in the browser and natively, and reads them from
        # the bundle `WCMP_ZENA_SCENARIOS` names. It holds the run to the
        # committed record, `tests/zena/record.txt`, and checks the
        # `compile` and `compose` stages on the bundle
        # `WCMP_ZENA_RECORD_CHECK` names.
        #
        # The fidelity suite of the runtime layer
        # (`rust/wcmp-wasm-core-fidelity`) embeds every script of the
        # pinned specification test suite at compile time, from the tree
        # `WCMP_SPEC_TESTSUITE` names, and generates one test for each.
        withTestInputs =
          derivation:
          derivation.overrideAttrs {
            WCMP_ZENA_SCENARIOS = "${zenaTestScenarios}/scenarios.bundle";
            WCMP_ZENA_RECORD_CHECK = "${zenaRecordCheck}/scenarios.bundle";
            WCMP_SPEC_TESTSUITE = "${spec-testsuite}";
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
        # - Two programs whose wiring file links them at run time pass only
        #   when the name crosses the link and the greeting crosses back.
        # - A program Zena refuses stops at `compile` and makes no call.
        zenaWasmtimeCheck =
          let
            cases = ./rust/wcmp/tests/zena/wasmtime-check;
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

            test "$(stage links-at-run-time)" = "stage pass"
            grep -qx 'call importer welcome("check") -> "hello, check!"' \
              ${observed}/links-at-run-time/observations.txt

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
        chrome = if pkgs.stdenv.hostPlatform.isDarwin then pkgs.google-chrome else pkgs.chromium;
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
        # board README, the benchmark README, and the Zena README; the board
        # file itself is machine-managed and stays out of the gate.
        markdown = katsuobushi.lib.markdown {
          inherit pkgs;
          workspaceRoot = ./.;
          include = [
            "CLAUDE.md"
            "README.md"
            "project/design/**/*.md"
            "project/kanban/README.md"
            "rust/wcmp-bench/README.md"
            "rust/wcmp/tests/zena/README.md"
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
          pname = "wcmp-public-api";
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
              cargo public-api --package wcmp -sss \
                --color never > public-api.txt
            '';
            installPhaseCommand = ''
              mkdir -p "$out"
              cp public-api.txt "$out/public-api.txt"
            '';
            doNotPostBuildInstallCargoBinaries = true;
          }
        );

        # The polyfill reaches its runtime layer through one seam module,
        # `src/runtime_layer.rs`, so moving to another runtime layer changes
        # one file. Every other source of the library names neither the
        # runtime layer's crate nor a backend crate, even in a comment. Two
        # places may name a backend, because the polyfill has none of its
        # own and a host always names one: the tests, and the examples of
        # the documentation, which run as tests.
        runtimeLayerSeamCheck =
          let
            sources = pkgs.lib.fileset.toSource {
              root = ./rust/wcmp;
              fileset = pkgs.lib.fileset.fileFilter (file: file.hasExt "rs") ./rust/wcmp;
            };
          in
          pkgs.runCommand "wcmp-runtime-layer-seam-check" { } ''
            cd ${sources}
            if grep -rn 'wcmp_wasm_core' src \
              | grep -v '^src/runtime_layer\.rs:' \
              | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//[/!]'; then
              echo >&2
              echo "Only src/runtime_layer.rs may name a runtime-layer crate." >&2
              echo "Reach the names above through crate::runtime_layer instead." >&2
              exit 1
            fi
            touch "$out"
          '';

        # The browser backend never makes a function from a string of
        # source, so it runs under a content security policy without
        # `unsafe-eval`. Its crate holds no JavaScript file, and no Rust
        # source of it names `eval`, the `Function` constructor or a way to
        # reach it (a string naming it, the `constructor` of another
        # function, or a binding whose `js_name` is `Function`), a timer
        # that takes a string of source, or an inline or module JavaScript
        # snippet of `wasm-bindgen`. The list cannot name every way there
        # is. The guard the browser itself enforces is the test that runs
        # the polyfill under the policy
        # (`rust/wcmp/tests/baseline_content_security_policy.rs`); this
        # check catches the plain ways before any test runs.
        webBackendNoEvalCheck =
          let
            crate = pkgs.lib.fileset.toSource {
              root = ./rust/wcmp-wasm-core-web;
              fileset = ./rust/wcmp-wasm-core-web;
            };
          in
          pkgs.runCommand "wcmp-web-backend-no-eval-check" { } ''
            cd ${crate}
            status=0
            if find . -name '*.js' -o -name '*.mjs' | grep .; then
              echo "The browser backend holds a JavaScript file." >&2
              status=1
            fi
            if grep -rnE '\beval\b|new_with_args|new_no_args|"Function"|Reflect::construct|inline_js|module *= *"|"constructor"|\.constructor\(|js_(name|class) *= *"?Function\b|set_?[tT]imeout|set_?[iI]nterval' src; then
              echo "The browser backend names a way to make a function from source." >&2
              status=1
            fi
            test "$status" = 0
            touch "$out"
          '';

        # crane over this workspace's toolchain, which carries the `wasm32`
        # target, for the two checks below that drive cargo themselves.
        publishCrane = (katsuobushi.inputs.crane.mkLib pkgs).overrideToolchain (_: rustToolchain);

        # The crates a consumer of the polyfill downloads: the polyfill, the
        # runtime layer and its two published backends, and the macros the
        # browser backend runs when it builds. The order is the order they
        # publish in: each comes after every crate it depends on.
        publishedCrates = [
          "wcmp-macros"
          "wcmp-wasm-core"
          "wcmp-wasm-core-wasmtime"
          "wcmp-wasm-core-web"
          "wcmp"
        ];

        # The version every published crate carries, the workspace's.
        publishedVersion = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;

        # `cargo package` for each published crate, natively and for
        # `wasm32-unknown-unknown`, where the browser backend has its code.
        # Nothing is published: the `.crate` files are the output.
        #
        # A package resolves its dependencies from the registry, and no
        # registry holds these crates yet. Cargo stands in for one with the
        # other packages of the same run, but not while crates.io is
        # replaced, as the vendored crates of this build replace it. So the
        # build keeps a registry directory of its own: the vendored crates,
        # and each crate as it packages, in the order they publish in. The
        # next crate verifies against that package, as it would against the
        # registry.
        packageCheck = publishCrane.mkCargoDerivation {
          pname = "wcmp-package";
          version = publishedVersion;
          src = katsuobushi.inputs.nix-filter.lib {
            root = ./.;
            include = [
              "Cargo.lock"
              "Cargo.toml"
              "rust"
            ];
          };
          strictDeps = true;
          nativeBuildInputs = commonBuildInputs;
          # The verify step builds each package as its own workspace, under
          # Cargo's default profile, so a dependency bundle of this
          # workspace would not be reused.
          cargoArtifacts = null;
          # With crates.io replaced by a directory, cargo cannot infer which
          # registry the packages are for, so it is told.
          buildPhaseCargoCommand = ''
            registry="$PWD/published-registry"
            mkdir -p "$registry"
            vendored=$(sed -n 's/^replace-with = "\(.*\)"$/\1/p' "$cargoVendorDir/config.toml")
            ln -s "$(sed -n "/^\[source\.$vendored\]/,/^\[/ s/^directory = \"\(.*\)\"$/\1/p" \
              "$cargoVendorDir/config.toml")"/* "$registry"/
            publish() {
              cargo \
                --config 'source.crates-io.replace-with = "published"' \
                --config "source.published.directory = \"$registry\"" \
                package --offline --registry crates-io "$@"
            }
            for crate in ${pkgs.lib.concatStringsSep " " publishedCrates}; do
              publish --package "$crate"
              package="$crate-${publishedVersion}"
              tar -xzf "target/package/$package.crate" -C "$registry"
              printf '{"files":{},"package":"%s"}' \
                "$(sha256sum "target/package/$package.crate" | cut -d ' ' -f 1)" \
                >"$registry/$package/.cargo-checksum.json"
            done
            publish --target wasm32-unknown-unknown \
              ${pkgs.lib.concatMapStringsSep " " (crate: "--package ${crate}") publishedCrates}
          '';
          installPhaseCommand = ''
            mkdir -p "$out"
            cp target/package/*.crate "$out"/
          '';
          doInstallCargoArtifacts = false;
          doNotPostBuildInstallCargoBinaries = true;
        };

        # A crate outside the workspace, `rust/wcmp-downstream`, which
        # depends on the polyfill and on a backend as a crates.io consumer
        # does, with its own lock file and no `[patch]` section. It builds
        # for the host and for `wasm32-unknown-unknown`. The source holds
        # the consumer, the published crates, and the workspace manifest
        # they inherit from, and nothing else of the workspace.
        downstreamBuildCheck =
          let
            lock = ./rust/wcmp-downstream/Cargo.lock;
          in
          publishCrane.mkCargoDerivation {
            pname = "wcmp-downstream-build";
            version = "0.1.0";
            src = pkgs.lib.fileset.toSource {
              root = ./.;
              fileset = pkgs.lib.fileset.unions (
                [
                  ./Cargo.toml
                  ./rust/wcmp-downstream
                ]
                ++ map (crate: ./rust + "/${crate}") publishedCrates
              );
            };
            cargoLock = lock;
            cargoVendorDir = publishCrane.vendorCargoDeps { cargoLock = lock; };
            postUnpack = ''
              cd "$sourceRoot/rust/wcmp-downstream"
              sourceRoot=.
            '';
            strictDeps = true;
            nativeBuildInputs = commonBuildInputs;
            cargoArtifacts = null;
            buildPhaseCargoCommand = ''
              if grep -nE '^[[:space:]]*(\[+[[:space:]]*patch\b|patch[[:space:]]*[.=])' Cargo.toml; then
                echo "The downstream crate has a [patch] section, which a" >&2
                echo "consumer from crates.io would not see." >&2
                exit 1
              fi
              cargo build --locked --offline
              cargo build --locked --offline --target wasm32-unknown-unknown
            '';
            installPhaseCommand = ''
              touch "$out"
            '';
            doInstallCargoArtifacts = false;
            doNotPostBuildInstallCargoBinaries = true;
          };

        # The listing against the one checked in beside the crate. A `pub`
        # that reaches a workspace-internal type — a field, a constructor, a
        # method whose type no caller outside can name — is a line in the
        # listing, so it arrives as a diff in review rather than as a piece
        # of surface nobody noticed was nameable.
        #
        # No public signature names a type of the runtime layer, so a host
        # names only the backend it chose. The one line that names the
        # runtime layer at all is the engine's constructor, whose parameter
        # is bounded by the trait every backend implements.
        publicApiCheck =
          pkgs.runCommand "wcmp-public-api-check" { }
            ''
              if grep -n 'wcmp_wasm_core' \
                ${publicApiListing}/public-api.txt \
                | grep -v 'pub fn wcmp::Engine::with_backend('; then
                echo >&2
                echo "A public signature above names the runtime layer. Only" >&2
                echo "Engine::with_backend may, through the bound of its backend." >&2
                exit 1
              fi
              if ! diff -u ${./rust/wcmp/public-api.txt} \
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
        # The conformance progress summary on one target: replays only the
        # summary test from the archive with its output shown. A native
        # run also writes the JSON copy under the cargo target directory,
        # `summary.json` on Wasmtime and `summary.wasmi.json` on Wasmi.
        # `backend` names the native backend the run hands its engines
        # (`WCMP_TEST_BACKEND`); the archive is the same for both.
        conformanceSummaryFor =
          {
            package,
            backend ? null,
          }:
          let
            label = if backend == null then package else "${package} on ${backend}";
            scratch = if backend == null then "${package}-summary" else "${package}-${backend}-summary";
            file = if backend == null then "summary.json" else "summary.${backend}.json";
          in
          ''
            echo "== conformance progress: ${label}"
            archive=$(nix build --no-link --print-out-paths .#${package})
            summary="''${CARGO_TARGET_DIR:-target}/conformance/${file}"
            mkdir -p "$(dirname "$summary")" "''${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/${scratch}"
          ''
          + pkgs.lib.optionalString (backend != null) ''
            export WCMP_TEST_BACKEND=${backend}
          ''
          + ''
            WCMP_CONFORMANCE_SUMMARY="$summary" cargo nextest run \
              --workspace-remap ./ \
              --archive-file "$archive/${package}.tar.zst" \
              --extract-to "''${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/${scratch}" \
              --extract-overwrite \
              --no-capture \
              -E 'test(=it_reports_conformance_progress)'
            rm -rf "''${XDG_CACHE_HOME:-$HOME/.cache}/wcmp-tests/${scratch}"
          ''
          + pkgs.lib.optionalString (backend != null) ''
            unset WCMP_TEST_BACKEND
          '';
        conformanceSummaryCommand = conformanceSummaryFor { package = "tests-native-debug"; };
        conformanceSummaryWasmiCommand = conformanceSummaryFor {
          package = "tests-native-debug";
          backend = "wasmi";
        };

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
        # (`WCMP_REGENERATE_BASE` names that list). A third run, on the
        # Wasmi backend, rewrites `expected-failures.wasmi.txt` from the
        # failures of that backend the shared list does not name. The runs
        # always write copies under the lane workspace and the diffs are
        # printed; `--dry-run` stops there, which is also how to read the
        # live reason of a directive a list already names.
        regenerateExpectationsCommand = ''
          corpus="$(git rev-parse --show-toplevel)"/rust/wcmp/tests/corpus
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
          wasmi="$workspace/expected-failures.wasmi.txt"
          cp "$corpus/expected-failures.txt" "$shared"
          cp "$corpus/expected-failures.no-provider.txt" "$overlay"
          cp "$corpus/expected-failures.wasmi.txt" "$wasmi"
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
          WCMP_TEST_BACKEND=wasmi WCMP_REGENERATE_EXPECTATIONS="$wasmi" \
            WCMP_REGENERATE_BASE="$shared" regenerate it_reports_conformance_progress
          for name in expected-failures.txt expected-failures.no-provider.txt \
            expected-failures.wasmi.txt; do
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

        # `tests zena` and `tests zena regenerate`: the Zena scenarios on
        # every subject, from the debug archives. One target cannot
        # run the other's subject, so each lane alone holds only the
        # Wasmtime line and its own polyfill line to the record. This
        # command runs the browser's scenario run first and keeps its
        # output, whose report lines the native `zena` test then reads
        # (`WCMP_ZENA_WEB_RUN`), adds the Wasmtime report and the reports
        # of the polyfill over the Wasmtime and the Wasmi backends to, and
        # holds to every line of the record. It writes the
        # compatibility report of the run (`WCMP_ZENA_REPORT`), which is
        # printed last, after a failure's differences too. `regenerate`
        # has the test write the record of the run
        # (`WCMP_ZENA_REGENERATE`), with the revision the build compiled
        # with, which is the flake lock's, in its header; the difference
        # from the committed record is printed, and `--dry-run` stops
        # there.
        zenaCommand = ''
          record="$(git rev-parse --show-toplevel)"/rust/wcmp/tests/zena/record.txt
          regenerate=""
          dry=""
          if [ "''${1:-}" = regenerate ]; then
            regenerate=1
            shift
            for argument in "$@"; do
              case "$argument" in
                --dry-run) dry=1 ;;
                *)
                  echo "tests zena regenerate: $argument is not an argument of this command (only --dry-run)" >&2
                  exit 2
                  ;;
              esac
            done
          elif [ "$#" -gt 0 ]; then
            echo "tests zena: $1 is not an argument of this command (only regenerate [--dry-run])" >&2
            exit 2
          fi
          web=$(nix build --no-link --print-out-paths .#tests-web-debug)
          native=$(nix build --no-link --print-out-paths .#tests-native-debug)
        ''
        + testWorkspace "tests-zena"
        + browserPool
        + ''
          replay() {
            cargo nextest run \
              --workspace-remap ./ \
              --archive-file "$1" \
              --extract-to "$workspace/archive" \
              --extract-overwrite \
              --ignore-default-filter \
              -E "binary(zena) and test(=$2)" \
              "''${@:3}"
          }
          echo "== the browser subject (wasm32-unknown-unknown, debug)"
          if ! replay "$web/tests-web-debug.tar.zst" \
            it_runs_every_zena_scenario_and_reports_a_stage_and_its_reason \
            --no-capture >"$workspace/web-run.txt" 2>&1; then
            cat "$workspace/web-run.txt"
            echo "tests zena: the browser's scenario run failed" >&2
            exit 1
          fi
          rm -rf "$workspace/archive"
          mkdir -p "$workspace/archive"
          echo "== the Wasmtime run and the native subjects over Wasmtime and Wasmi (${system}, debug)"
          export WCMP_ZENA_WEB_RUN="$workspace/web-run.txt"
          export WCMP_ZENA_REPORT="$workspace/report.txt"
          if [ -n "$regenerate" ]; then
            export WCMP_ZENA_REGENERATE="$workspace/record.txt"
          fi
          status=0
          replay "$native/tests-native-debug.tar.zst" \
            it_holds_every_subject_to_the_record_and_prints_the_report || status=$?
          if [ -s "$WCMP_ZENA_REPORT" ]; then
            echo
            cat "$WCMP_ZENA_REPORT"
          fi
          if [ "$status" != 0 ]; then
            exit "$status"
          fi
          if [ -n "$regenerate" ]; then
            echo
            if diff -u "$record" "$WCMP_ZENA_REGENERATE"; then
              echo "tests zena regenerate: tests/zena/record.txt is current, nothing to write"
            elif [ -n "$dry" ]; then
              echo "tests zena regenerate: the diff above is what a run would write; tests/zena/record.txt is unchanged"
            else
              install -m 644 "$WCMP_ZENA_REGENERATE" "$record"
              echo "tests zena regenerate: wrote $record"
            fi
          fi
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
            # A filterset that narrows the profile's tests further.
            filter ? null,
            # The native backend the polyfill's tests hand their engines
            # (`WCMP_TEST_BACKEND`), when it is not Wasmtime. A native
            # archive runs on either backend, so a lane of another
            # backend replays the build of the Wasmtime lane.
            backend ? null,
          }:
          let
            # Lanes that replay one archive under different profiles or
            # on different backends get workspaces of their own.
            lane =
              pkgs.lib.concatStringsSep "-" (
                [ package ]
                ++ pkgs.lib.optional (backend != null) backend
                ++ pkgs.lib.optional (nextestProfile != "default") nextestProfile
              );
          in
          {
            inherit description;
            command = ''
              archive=$(nix build --no-link --print-out-paths .#${package})
            ''
            + testWorkspace lane
            + pkgs.lib.optionalString browser (browserPool + browserTestThreads)
            + pkgs.lib.optionalString (backend != null) ''
              export WCMP_TEST_BACKEND=${backend}
            ''
            + ''
              cargo nextest run \
                --profile ${nextestProfile} \
                --workspace-remap ./ \
                --archive-file "$archive/${package}.tar.zst" \
                --extract-to "$workspace/archive" \
                --extract-overwrite \
            ''
            + pkgs.lib.optionalString (filter != null) ''
              -E ${pkgs.lib.escapeShellArg filter} \
            ''
            + ''
                "$@"
            ''
            # The summary always replays the native debug archive, on the
            # lane's backend.
            + pkgs.lib.optionalString summary (conformanceSummaryFor {
              package = "tests-native-debug";
              inherit backend;
            });
          };

        # `cache push`: see `nix/cache-push.sh`. Nix itself comes from the
        # caller's PATH, so the push instantiates with the same Nix and
        # store settings as the builds it pushes.
        cachePush = pkgs.writeShellApplication {
          name = "wcmp-cache-push";
          runtimeInputs = with pkgs; [
            cachix
            coreutils
            findutils
            gawk
            git
            gnugrep
            jq
          ];
          text = builtins.readFile ./nix/cache-push.sh;
        };

        # Every menu command but the sandbox's, which is the whole menu of
        # the `ci` shell: the sandbox commands reference the guest VM, and a
        # shell that carries them has the guest's closure to realize.
        ciCommands = {
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
                description = "Run the benchmark suite on ${system} over the Wasmtime backend";
                command = ''
                  report="''${CARGO_TARGET_DIR:-target}/bench/native.json"
                  mkdir -p "$(dirname "$report")"
                  binary=$(nix build --no-link --print-out-paths .#bench-native)
                  WCMP_BENCH_REPORT="$report" "$binary"/bin/wcmp-bench "$@"
                '';
              };
              wasmi = {
                description = "Run the benchmark suite on ${system} over the Wasmi backend, with the binary `bench native` builds";
                command = ''
                  report="''${CARGO_TARGET_DIR:-target}/bench/wasmi.json"
                  mkdir -p "$(dirname "$report")"
                  binary=$(nix build --no-link --print-out-paths .#bench-native)
                  WCMP_BENCH_BACKEND=wasmi WCMP_BENCH_REPORT="$report" "$binary"/bin/wcmp-bench "$@"
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
            description = "Run the test suites from Nix-built archives, and the Zena scenarios with their compatibility report";
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
                  # The polyfill's tests on the Wasmi backend, from the
                  # archive the debug lane builds. The runtime layer's own
                  # crates choose no backend at run time, so only the
                  # polyfill's tests run again. Its profile runs the tests
                  # that hold memories of several GiB on Wasmi one at a
                  # time.
                  wasmi = menuTestCommand {
                    description = "The polyfill's unit and integration tests on the Wasmi backend, from the debug lane's build (${system}, debug)";
                    package = "tests-native-debug";
                    nextestProfile = "wasmi";
                    backend = "wasmi";
                    filter = "package(wcmp)";
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
                command =
                  conformanceSummaryCommand
                  + conformanceSummaryWasmiCommand
                  + conformanceSummaryFor { package = "tests-web-debug"; };
              };
              # The fidelity suite of the runtime layer: the pinned
              # specification test suite on one backend, through the
              # `wcmp-wasm-core` trait alone, held to the backend's cited
              # expected failures. Each backend is a leaf of its own.
              fidelity = {
                description = "The pinned WebAssembly specification test suite on one backend of the runtime layer, for the floor and each capability the backend declares, then the suite of every script";
                subcommands = {
                  wasmi = menuTestCommand {
                    description = "The fidelity suite on the Wasmi backend (${system}, debug)";
                    package = "tests-native-debug";
                    nextestProfile = "fidelity";
                    filter = "binary_id(wcmp-wasm-core-wasmi::fidelity)";
                  };
                  wasmtime = menuTestCommand {
                    description = "The fidelity suite on the Wasmtime backend (${system}, debug)";
                    package = "tests-native-debug";
                    nextestProfile = "fidelity";
                    filter = "binary_id(wcmp-wasm-core-wasmtime::fidelity)";
                  };
                  web = menuTestCommand {
                    description = "The fidelity suite on the browser backend (wasm32-unknown-unknown, debug)";
                    package = "tests-web-debug";
                    nextestProfile = "fidelity";
                    filter = "binary_id(wcmp-wasm-core-web::fidelity)";
                    browser = true;
                  };
                };
              };
              regenerate = {
                description = "Rewrite the shared expected-failure list from the native run with the suspend provider, the no-provider overlay from the native run without it, and the Wasmi delta from the run on the Wasmi backend (`--dry-run` only prints the diffs)";
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
              zena = {
                description = "The Zena scenarios on every subject, held to every line of tests/zena/record.txt, then the compatibility report: the pin, each scenario with the stage of Browser, Native (the polyfill over Wasmtime), Wasmi, and Wasmtime and the reason of each that stopped before pass, and pass counts (`tests zena regenerate [--dry-run]` writes the record again from every subject, the browser included, or only prints the difference)";
                command = zenaCommand;
              };
              # The conformance corpus runs in four states: each target with
              # the suspend provider allowed (the debug and release lanes)
              # and with it turned off (the `no-provider` lanes). The Wasmi
              # lane replays the native debug archive on the Wasmi backend,
              # so it adds no build. The Zena lane is the one run that holds
              # the browser's line and both native lines of the record
              # together. The fidelity lane runs the specification test
              # suite on each backend of the runtime layer. Each lane reports
              # its wall-clock time, build included.
              # Arguments after the leaf reach every nextest lane, and not
              # the Zena lane, which takes none.
              all = {
                description = "Every lane, each timed: both targets in debug and release, the conformance corpus on both targets with the suspend provider off, so the corpus runs in all four states, the polyfill's tests and the corpus on the Wasmi backend, the fidelity suite on each backend of the runtime layer, and the Zena scenarios on every subject (grab a coffee)";
                command = ''
                  status=0
                  lane() {
                    local suite=$1
                    shift
                    started=$SECONDS
                    # shellcheck disable=SC2086
                    if tests $suite "$@"; then
                      echo "tests $suite: passed in $((SECONDS - started))s"
                    else
                      echo "tests $suite: FAILED after $((SECONDS - started))s"
                      status=1
                    fi
                  }
                  for suite in "native debug" "native release" "native no-provider" \
                    "native wasmi" "web debug" "web release" "web no-provider" "fidelity wasmi" \
                    "fidelity wasmtime" "fidelity web"; do
                    lane "$suite" "$@"
                  done
                  lane zena
                  exit "$status"
                '';
              };
            };
          };

          # Arguments after `lint` reach `nix flake check`: CI passes `-L
          # --keep-going`, so its log carries each build's output and one
          # failed check does not stop the others.
          "lint" = {
            description = "Every check the flake declares (nix flake check; arguments pass through)";
            command = ''
              nix flake check "$@"
            '';
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
                  snapshot="$(git rev-parse --show-toplevel)"/rust/wcmp/public-api.txt
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
              "$(git rev-parse --show-toplevel)"/rust/wcmp/tests/corpus/fixtures/build.sh
            '';
          };

          # The project's binary cache, the one `nixConfig` names and CI
          # pushes to.
          "cache" = {
            description = "Work with the project's binary cache (wcmp.cachix.org)";
            subcommands = {
              push = {
                description = "Push what this machine built for CI (checks, the ci shell, the test archives' inputs), never the test archives or Claude Code; asks for a token if none is stored (`--dry-run` only summarizes, `--yes` skips the question)";
                command = ''
                  WCMP_CACHE=wcmp WCMP_SYSTEM=${system} ${cachePush}/bin/wcmp-cache-push "$@"
                '';
              };
            };
          };

        }
        // markdown.menuCommands
        // project.menuCommands;

        commands =
          ciCommands
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
              # A native run takes seconds; a hung one fails the check here
              # rather than holding the build until a CI job's time limit.
              if ! transcript=$(timeout 600 ${smokeNative}/bin/wcmp-smoke); then
                echo "the native smoke run failed or did not finish within 600s" >&2
                exit 1
              fi
              native=$(printf '%s\n' "$transcript" | tail -n 1)
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
            pname = "wcmp";
            version = "0.1.0";
            cargoExtraArgs = "--package wcmp";
            inherit target profile;
          };

        # Bound here so the `workspace-deps-dev` package below can name the
        # same derivation the archive builds against.
        testsNativeDebug = withTestInputs (buildTestArchive {
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
        ciMenu = makeMenu {
          title = "WCMP (CI)";
          commands = ciCommands;
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
          # The scenarios' Rust partners alone, built with this workspace's
          # Rust toolchain.
          zena-partners = zenaPartners;
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

          tests-native-release = withTestInputs (buildTestArchive {
            name = "native-release";
          });

          tests-web-debug = withTestInputs (buildTestArchive {
            name = "web-debug";
            target = "wasm32-unknown-unknown";
            profile = "dev";
          });

          tests-web-release = withTestInputs (buildTestArchive {
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
            # Clippy compiles every target, the `zena` test and the
            # fidelity suite included, and both embed their inputs at
            # compile time.
            clippy = withTestInputs cargoChecks.clippy;
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
            # Only the seam module names a runtime-layer crate: see
            # `runtimeLayerSeamCheck`.
            runtime-layer-seam = runtimeLayerSeamCheck;
            # The browser backend makes no function from a string of
            # source: see `webBackendNoEvalCheck`.
            web-backend-no-eval = webBackendNoEvalCheck;
            # Every published crate packages, and a crate outside the
            # workspace builds against the polyfill and a backend with no
            # `[patch]` section: see `packageCheck` and
            # `downstreamBuildCheck`.
            package = packageCheck;
            downstream-build = downstreamBuildCheck;
            # Every Zena scenario compiles, or keeps Zena's refusal, with the
            # pinned toolchain, its Rust partners build, and its
            # composition is made or keeps `wac`'s refusal; and the
            # scenario build keeps a refusal as its output: see
            # `compileScenarios` and `buildZenaScenarios`.
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
              pname = "wcmp-doctests";
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

        # The shell GitHub Actions runs the menu in: the same tools and
        # commands, without the sandbox's, so a runner realizes neither the
        # guest VM nor `katsuctl` before a lane can start.
        devShells.ci = pkgs.mkShell {
          name = "wcmp-ci";
          env = developmentEnvVars;
          nativeBuildInputs = ciMenu.commands ++ developmentBuildInputs;
          shellHook = rustEnvironmentHook + makeDevShellHook ciMenu;
        };
      }
    );
}
