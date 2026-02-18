_: {
  perSystem = {config, ...}: let
    inherit (config.idm) pkgs craneLib commonArgs cargoArtifacts packagePreCheck;

    idm = craneLib.buildPackage (commonArgs
      // {
        inherit cargoArtifacts;
        preCheck = packagePreCheck;
      });

    vhs = pkgs.writeShellApplication {
      name = "idm-demo";
      runtimeInputs = [pkgs.vhs idm];
      text = ''
        wrapper_dir="$PWD/.idm-demo-bin"
        mkdir -p "$wrapper_dir"
        cat > "$wrapper_dir/idm" <<'EOF'
        #!/usr/bin/env bash
        exec ${idm}/bin/idm \
          --fake \
          --fake-scan "hci0|AA:BB:CC:DD:EE:FF|IDM-Clock|-43" \
          --fake-discovery-delay 2s \
          "$@"
        EOF
        chmod +x "$wrapper_dir/idm"

        PATH="$wrapper_dir:$PATH"
        vhs ${../demo.tape}
      '';
    };
  in {
    packages = {
      inherit idm vhs;
      default = idm;
    };
  };
}
