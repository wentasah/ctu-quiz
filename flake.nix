{
  description = "Quiz web app: shows a Markdown question to students and collects their answers";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" "aarch64-linux" "x86_64-darwin" "aarch64-darwin" ];
      forAllSystems = f:
        nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs: rec {
        quiz = pkgs.rustPlatform.buildRustPackage {
          pname = "quiz";
          version = (builtins.fromTOML (builtins.readFile ./Cargo.toml)).workspace.package.version;
          src = self;
          # No vendor hash needed: the lock file pins everything.
          cargoLock.lockFile = ./Cargo.lock;

          nativeBuildInputs = [ pkgs.installShellFiles ];

          # `cli/build.rs` writes the completions here while it compiles.
          preBuild = ''
            export QUIZ_COMPLETIONS_DIR="$NIX_BUILD_TOP/completions"
          '';

          postInstall = ''
            installShellCompletion \
              --bash "$QUIZ_COMPLETIONS_DIR/quiz.bash" \
              --zsh  "$QUIZ_COMPLETIONS_DIR/_quiz" \
              --fish "$QUIZ_COMPLETIONS_DIR/quiz.fish"
          '';

          # The tests start the server and a mock OIDC provider on loopback,
          # which works inside the build sandbox.
          meta = with pkgs.lib; {
            description = "Quiz server and teacher CLI for asking students questions";
            mainProgram = "quiz-server";
            license = licenses.mit;
            platforms = platforms.unix;
          };
        };
        default = quiz;
      });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          packages = with pkgs; [
            cargo
            rustc
            clippy
            rustfmt
            rust-analyzer
            openssl # only for generating a test key
          ];
          RUST_BACKTRACE = "1";
        };
      });

      nixosModules.default = { config, lib, pkgs, ... }:
        let
          cfg = config.services.quiz-server;
        in
        {
          options.services.quiz-server = {
            enable = lib.mkEnableOption "the quiz server";

            package = lib.mkOption {
              type = lib.types.package;
              default = self.packages.${pkgs.stdenv.hostPlatform.system}.quiz;
              description = "Package providing quiz-server.";
            };

            listen = lib.mkOption {
              type = lib.types.str;
              default = "127.0.0.1:8080";
              description = "Address the server listens on, usually behind a reverse proxy.";
            };

            publicUrl = lib.mkOption {
              type = lib.types.str;
              example = "https://quiz.iid.ciirc.cvut.cz";
              description = ''
                Public URL of the server. It ends up in the QR code and in the
                OIDC redirect URI, so it has to match the reverse proxy.
              '';
            };

            issuer = lib.mkOption {
              type = lib.types.str;
              example = "https://idp.example.org/realms/fel";
              description = "URL of the OIDC provider.";
            };

            clientId = lib.mkOption {
              type = lib.types.str;
              default = "quiz";
              description = "OIDC client ID of the server.";
            };

            cliClientId = lib.mkOption {
              type = lib.types.nullOr lib.types.str;
              default = null;
              example = "quiz-cli";
              description = ''
                OIDC client ID of the teacher CLI, if it is registered as a
                separate public client. Null means the same as clientId.
              '';
            };

            teachers = lib.mkOption {
              type = lib.types.listOf lib.types.str;
              default = [ ];
              example = [ "sojkam1" ];
              description = ''
                Logins allowed to ask questions. The server refuses to start
                with an empty list unless allowAnyTeacher is set.
              '';
            };

            allowAnyTeacher = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = ''
                Let everybody who can log in with the identity provider ask
                questions, instead of only the logins in teachers.
              '';
            };

            trustProxy = lib.mkOption {
              type = lib.types.bool;
              default = true;
              description = "Log the client address from X-Forwarded-For.";
            };

            environmentFile = lib.mkOption {
              type = lib.types.nullOr lib.types.path;
              default = null;
              example = "/run/secrets/quiz-server.env";
              description = ''
                File with additional environment variables, read by systemd.
                This is where QUIZ_OIDC_CLIENT_SECRET belongs, so that the
                secret stays out of the world-readable Nix store.
              '';
            };

            extraEnvironment = lib.mkOption {
              type = lib.types.attrsOf lib.types.str;
              default = { };
              example = { RUST_LOG = "info,quiz_server=debug"; };
              description = "Further environment variables for the service.";
            };
          };

          config = lib.mkIf cfg.enable {
            systemd.services.quiz-server = {
              description = "Quiz server";
              wantedBy = [ "multi-user.target" ];
              after = [ "network-online.target" ];
              wants = [ "network-online.target" ];

              environment = {
                QUIZ_LISTEN = cfg.listen;
                QUIZ_PUBLIC_URL = cfg.publicUrl;
                QUIZ_OIDC_ISSUER = cfg.issuer;
                QUIZ_OIDC_CLIENT_ID = cfg.clientId;
                QUIZ_TEACHERS = lib.concatStringsSep "," cfg.teachers;
                QUIZ_ALLOW_ANY_TEACHER = lib.boolToString cfg.allowAnyTeacher;
                QUIZ_TRUST_PROXY = lib.boolToString cfg.trustProxy;
                QUIZ_LOG_FILE = "/var/log/quiz-server/quiz-server.log";
              } // lib.optionalAttrs (cfg.cliClientId != null) {
                QUIZ_OIDC_CLI_CLIENT_ID = cfg.cliClientId;
              } // cfg.extraEnvironment;

              serviceConfig = {
                Type = "exec";
                ExecStart = lib.getExe' cfg.package "quiz-server";
                EnvironmentFile = lib.mkIf (cfg.environmentFile != null) cfg.environmentFile;
                Restart = "on-failure";
                RestartSec = "5s";

                # The server keeps nothing on disc except its log.
                DynamicUser = true;
                LogsDirectory = "quiz-server";
                LogsDirectoryMode = "0750";

                NoNewPrivileges = true;
                CapabilityBoundingSet = "";
                AmbientCapabilities = "";
                PrivateDevices = true;
                ProtectClock = true;
                ProtectControlGroups = true;
                ProtectHome = true;
                ProtectHostname = true;
                ProtectKernelLogs = true;
                ProtectKernelModules = true;
                ProtectKernelTunables = true;
                ProtectProc = "invisible";
                ProtectSystem = "strict";
                RestrictAddressFamilies = [ "AF_INET" "AF_INET6" ];
                RestrictNamespaces = true;
                RestrictRealtime = true;
                RestrictSUIDSGID = true;
                LockPersonality = true;
                MemoryDenyWriteExecute = true;
                SystemCallArchitectures = "native";
                SystemCallFilter = [ "@system-service" ];
                UMask = "0077";
              };
            };
          };
        };
    };
}
