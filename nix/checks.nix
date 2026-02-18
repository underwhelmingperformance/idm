_: {
  perSystem = {config, ...}: let
    inherit (config.idm) craneLib commonArgs cargoArtifacts packagePreCheck;
  in {
    checks = {
      clippy = craneLib.cargoClippy (commonArgs
        // {
          inherit cargoArtifacts;
          cargoClippyExtraArgs = "--all-targets -- -D warnings";
        });

      tests = craneLib.cargoTest (commonArgs
        // {
          inherit cargoArtifacts;
          preCheck = packagePreCheck;
        });

      doc = craneLib.cargoDoc (commonArgs
        // {
          inherit cargoArtifacts;
        });

      build = config.packages.idm;
    };
  };
}
