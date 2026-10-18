# NixOS module running the quiz server as a hardened systemd service.
#
# The flake exports it as nixosModules.default, with the package built from
# the flake's own nixpkgs; imported directly, the package is built from the
# system's nixpkgs.
{ config, lib, pkgs, ... }:
let
  cfg = config.services.quiz-server;
in
{
  options.services.quiz-server = {
    enable = lib.mkEnableOption "the quiz server";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ./package.nix { };
      defaultText = lib.literalExpression "pkgs.callPackage ./package.nix { }";
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

    cliRedirectUri = lib.mkOption {
      type = lib.types.nullOr lib.types.str;
      default = null;
      example = "http://127.0.0.1:28191/callback";
      description = ''
        Redirect URI registered for the teacher CLI, for providers
        that do not accept any loopback port. Null lets the CLI pick
        any http://127.0.0.1:<port>/callback.
      '';
    };

    scopes = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ "openid" "profile" "email" ];
      example = [ "openid" "profile" ];
      description = ''
        OIDC scopes requested by the server and the teacher CLI. The
        provider refuses the whole login if any of them is not
        allowed for the client.
      '';
    };

    teachers = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "sojkam1" ];
      description = ''
        Logins allowed to ask questions. The server refuses to start
        with an empty list and no teacherRoles unless allowAnyTeacher
        is set.
      '';
    };

    teacherRoles = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "teacher" ];
      description = ''
        Roles allowed to ask questions: anybody whose access token
        lists one of them in the rolesClaim claim is a teacher.
      '';
    };

    rolesClaim = lib.mkOption {
      type = lib.types.str;
      default = "roles";
      description = ''
        Claim of the access token with the user's roles, a string or
        an array of strings.
      '';
    };

    allowAnyTeacher = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Let everybody who can log in with the identity provider ask
        questions, instead of only the logins in teachers and the
        roles in teacherRoles.
      '';
    };

    trustProxy = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Log the client address from X-Forwarded-For.";
    };

    saveState = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Save the rooms, questions and answers to
        /var/lib/quiz-server/state.json when the service stops, and
        read them back when it starts, so that a restart in the middle
        of a lecture loses nothing.
      '';
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
        QUIZ_OIDC_SCOPES = lib.concatStringsSep "," cfg.scopes;
        QUIZ_TEACHERS = lib.concatStringsSep "," cfg.teachers;
        QUIZ_TEACHER_ROLES = lib.concatStringsSep "," cfg.teacherRoles;
        QUIZ_ROLES_CLAIM = cfg.rolesClaim;
        QUIZ_ALLOW_ANY_TEACHER = lib.boolToString cfg.allowAnyTeacher;
        QUIZ_TRUST_PROXY = lib.boolToString cfg.trustProxy;
        QUIZ_LOG_FILE = "/var/log/quiz-server/quiz-server.log";
      } // lib.optionalAttrs (cfg.cliClientId != null) {
        QUIZ_OIDC_CLI_CLIENT_ID = cfg.cliClientId;
      } // lib.optionalAttrs cfg.saveState {
        QUIZ_STATE_FILE = "/var/lib/quiz-server/state.json";
      } // lib.optionalAttrs (cfg.cliRedirectUri != null) {
        QUIZ_OIDC_CLI_REDIRECT_URI = cfg.cliRedirectUri;
      } // cfg.extraEnvironment;

      serviceConfig = {
        Type = "exec";
        ExecStart = lib.getExe' cfg.package "quiz-server";
        EnvironmentFile = lib.mkIf (cfg.environmentFile != null) cfg.environmentFile;
        Restart = "on-failure";
        RestartSec = "5s";

        # The server keeps nothing on disc except its log, and the
        # state file if saveState is on.
        DynamicUser = true;
        LogsDirectory = "quiz-server";
        LogsDirectoryMode = "0750";
        StateDirectory = lib.mkIf cfg.saveState "quiz-server";

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
}
