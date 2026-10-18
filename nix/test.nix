# NixOS VM test of the module: the server starts against the mock OIDC
# provider shipped in the package, serves its pages behind a login, logs to
# its log file and, with saveState, saves its state when it is restarted.
{ module }:
{
  name = "quiz-server";

  nodes.machine = { config, pkgs, ... }: {
    imports = [ module ];

    systemd.services.mock-idp = {
      wantedBy = [ "multi-user.target" ];
      serviceConfig = {
        ExecStart = "${config.services.quiz-server.package}/bin/mock-idp --listen 127.0.0.1:9000";
        DynamicUser = true;
      };
    };

    services.quiz-server = {
      enable = true;
      publicUrl = "http://localhost:8080";
      issuer = "http://127.0.0.1:9000";
      teachers = [ "sojkam1" ];
      saveState = true;
    };
    # The server discovers the provider when it starts.
    systemd.services.quiz-server = {
      after = [ "mock-idp.service" ];
      requires = [ "mock-idp.service" ];
    };

    environment.systemPackages = [ pkgs.curl ];
  };

  testScript = ''
    machine.wait_for_unit("quiz-server.service")
    machine.wait_for_open_port(8080)
    machine.succeed("curl -sf http://localhost:8080/healthz | grep -x ok")

    # Students are sent to the provider to log in.
    location = machine.succeed(
        "curl -s -o /dev/null -w '%{redirect_url}' http://localhost:8080/"
    )
    assert "/login" in location, location
    location = machine.succeed(
        "curl -s -o /dev/null -w '%{redirect_url}' http://localhost:8080/login"
    )
    assert location.startswith("http://127.0.0.1:9000/"), location

    # The teacher API refuses requests without a token.
    machine.succeed(
        "curl -s -o /dev/null -w '%{http_code}' http://localhost:8080/api/v1/whoami | grep -x 401"
    )

    machine.succeed("grep -q 'starting the quiz server' /var/log/quiz-server/quiz-server.log")

    # The state is saved on stop and read back on start.
    machine.fail("test -e /var/lib/quiz-server/state.json")
    machine.systemctl("restart quiz-server.service")
    machine.wait_for_open_port(8080)
    machine.succeed("test -s /var/lib/quiz-server/state.json")
    machine.succeed("grep -q 'state of the previous run restored' /var/log/quiz-server/quiz-server.log")
  '';
}
