---
sidebar_position: 6
title: Updating ponduin
sidebar_label: Updating ponduin
---

import Tabs from '@theme/Tabs';
import TabItem from '@theme/TabItem';
import { DesktopAutoUpdateSteps } from '@site/src/components/DesktopAutoUpdateSteps';
import MacDesktopInstallButtons from '@site/src/components/MacDesktopInstallButtons';
import WindowsDesktopInstallButtons from '@site/src/components/WindowsDesktopInstallButtons';
import LinuxDesktopInstallButtons from '@site/src/components/LinuxDesktopInstallButtons';

The ponduin CLI and desktop apps are under active and continuous development. To get the newest features and fixes, you should periodically update your ponduin client using the following instructions.

<Tabs>
  <TabItem value="mac" label="macOS" default>
    <Tabs groupId="interface">
      <TabItem value="ui" label="ponduin Desktop" default>
        Update ponduin to the latest stable version.

        <DesktopAutoUpdateSteps />

        **To manually download and install updates:**
        1. <MacDesktopInstallButtons/>
        2. Unzip the downloaded zip file
        3. Drag the extracted `Ponduin.app` file to the `Applications` folder to overwrite your current version
        4. Launch ponduin Desktop

      </TabItem>
      <TabItem value="cli" label="ponduin CLI">
        You can update ponduin by running:

        ```sh
        ponduin update
        ```

        Additional [options](/docs/guides/ponduin-cli-commands#update-options):

        ```sh
        # Update to latest canary (development) version
        ponduin update --canary

        # Update and reconfigure settings
        ponduin update --reconfigure
        ```

        Or you can run the [installation](/docs/getting-started/installation) script again:

        ```sh
        curl -fsSL https://github.com/PondSec/ponduin/releases/download/stable/download_cli.sh | CONFIGURE=false bash
        ```

        To check your current ponduin version, use the following command:

        ```sh
        ponduin --version
        ```
      </TabItem>
    </Tabs>
  </TabItem>

  <TabItem value="linux" label="Linux">
    <Tabs groupId="interface">
      <TabItem value="ui" label="ponduin Desktop" default>
        Update ponduin to the latest stable version.

        <DesktopAutoUpdateSteps />

        **To manually download and install updates:**
        1. <LinuxDesktopInstallButtons/>

        #### For Debian/Ubuntu-based distributions
        2. In a terminal, navigate to the downloaded DEB file
        3. Run `sudo dpkg -i (filename).deb`
        4. Launch ponduin from the app menu
      </TabItem>
      <TabItem value="cli" label="ponduin CLI">
        You can update ponduin by running:

        ```sh
        ponduin update
        ```

        Additional [options](/docs/guides/ponduin-cli-commands#update-options):

        ```sh
        # Update to latest canary (development) version
        ponduin update --canary

        # Update and reconfigure settings
        ponduin update --reconfigure
        ```

        Or you can run the [installation](/docs/getting-started/installation) script again:

        ```sh
        curl -fsSL https://github.com/PondSec/ponduin/releases/download/stable/download_cli.sh | CONFIGURE=false bash
        ```

        To check your current ponduin version, use the following command:

        ```sh
        ponduin --version
        ```
      </TabItem>
    </Tabs>
  </TabItem>

  <TabItem value="windows" label="Windows">
    <Tabs groupId="interface">
      <TabItem value="ui" label="ponduin Desktop" default>
        Update ponduin to the latest stable version.

        <DesktopAutoUpdateSteps />

        **To manually download and install updates:**
        1. <WindowsDesktopInstallButtons/>
        2. Unzip the downloaded zip file
        3. Run the executable file to launch the ponduin Desktop app
      </TabItem>
      <TabItem value="cli" label="ponduin CLI">
        You can update ponduin by running:

        ```sh
        ponduin update
        ```

        Additional [options](/docs/guides/ponduin-cli-commands#update-options):

        ```sh
        # Update to latest canary (development) version
        ponduin update --canary

        # Update and reconfigure settings
        ponduin update --reconfigure
        ```

        Or you can run the [installation](/docs/getting-started/installation) script again in **Git Bash**, **MSYS2**, or **PowerShell** to update the ponduin CLI natively on Windows:

        ```bash
        curl -fsSL https://github.com/PondSec/ponduin/releases/download/stable/download_cli.sh | CONFIGURE=false bash
        ```

        To check your current ponduin version, use the following command:

        ```sh
        ponduin --version
        ```

        <details>
        <summary>Update via Windows Subsystem for Linux (WSL)</summary>

        To update your WSL installation, use `ponduin update` or run the installation script again via WSL:

        ```sh
        curl -fsSL https://github.com/PondSec/ponduin/releases/download/stable/download_cli.sh | CONFIGURE=false bash
        ```

       </details>
      </TabItem>
    </Tabs>
  </TabItem>
</Tabs>

:::info Updating in CI/CD
If you're running ponduin in CI or other non-interactive environments, pin a specific version with `PONDUIN_VERSION` for reproducible installs. See [CI/CD Environments](/docs/tutorials/cicd) for a complete example and usage details.
:::
