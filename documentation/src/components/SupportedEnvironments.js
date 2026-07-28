import React from "react";
import Admonition from "@theme/Admonition";

const SupportedEnvironments = () => {
  return (
    <Admonition type="info" title="Supported Environments">
      The ponduin CLI currently works on <strong>macOS</strong> and <strong>Linux</strong> systems and supports both <strong>ARM</strong> and <strong>x86</strong> architectures.
      On <strong>Windows</strong>, ponduin CLI can run via WSL, and ponduin Desktop is natively supported. If you'd like to request support for additional operating systems, please{" "}
      <a
        href="https://github.com/PondSec/ponduin/discussions/867"
        target="_blank"
        rel="noopener noreferrer"
      >
        vote on GitHub
      </a>.
    </Admonition>
  );
};

export default SupportedEnvironments;
