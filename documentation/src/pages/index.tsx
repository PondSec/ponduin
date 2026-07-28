import type { ReactNode } from "react";
import Link from "@docusaurus/Link";
import useDocusaurusContext from "@docusaurus/useDocusaurusContext";
import Layout from "@theme/Layout";

import styles from "./index.module.css";
import { PonduinLogo } from "../components/PonduinLogo";

function HeroSection() {
  return (
    <header className={styles.hero}>
      <div className={styles.heroInner}>
        <div className={styles.heroBadge}>
          Local · Privacy First · PondSec
        </div>
        <div className={styles.heroLogo}>
          <PonduinLogo />
        </div>
        <p className={styles.heroSubtitle}>
          A precise local AI agent for coding, terminal work, file management
          and powerful automation — with your data and tools under your control.
        </p>
        <div className={styles.heroActions}>
          <Link
            className="button button--primary button--lg"
            to="docs/getting-started/installation"
          >
            Install Ponduin
          </Link>
          <Link
            className={`button button--outline button--lg ${styles.secondaryButton}`}
            to="docs/quickstart"
          >
            Quickstart
          </Link>
        </div>
        <div className={styles.heroStats}>
          <div className={styles.stat}>
            <span className={styles.statNumber}>Local</span>
            <span className={styles.statLabel}>execution</span>
          </div>
          <div className={styles.statDivider} />
          <div className={styles.stat}>
            <span className={styles.statNumber}>Private</span>
            <span className={styles.statLabel}>by design</span>
          </div>
          <div className={styles.statDivider} />
          <div className={styles.stat}>
            <span className={styles.statNumber}>Flexible</span>
            <span className={styles.statLabel}>model choice</span>
          </div>
        </div>
      </div>
    </header>
  );
}

type FeatureCardProps = {
  title: string;
  description: ReactNode;
  icon: string;
};

function FeatureCard({ title, description, icon }: FeatureCardProps) {
  return (
    <div className={styles.featureCard}>
      <div className={styles.featureIcon}>{icon}</div>
      <h3 className={styles.featureTitle}>{title}</h3>
      <div className={styles.featureDescription}>{description}</div>
    </div>
  );
}

type SmallCardProps = {
  title: string;
  description: ReactNode;
  icon: string;
};

function SmallCard({ title, description, icon }: SmallCardProps) {
  return (
    <div className={styles.smallCard}>
      <div className={styles.smallCardIcon}>{icon}</div>
      <h3 className={styles.smallCardTitle}>{title}</h3>
      <div className={styles.smallCardDescription}>{description}</div>
    </div>
  );
}

function FeaturesSection() {
  return (
    <section className={styles.section}>
      <div className={styles.container}>
        <h2 className={styles.sectionTitle}>What Ponduin does</h2>
        <p className={styles.sectionSubtitle}>
          Ponduin is a general-purpose AI agent that runs on your machine. Not
          just for code — use it for research, writing, automation, data
          analysis, or anything you need to get done.
        </p>
        <div className={styles.featuresGridTop}>
          <FeatureCard
            icon="🖥️"
            title="Desktop app, CLI, and API"
            description={
              <p>
                A native desktop app for macOS, Linux, and Windows. A full CLI
                for terminal workflows. An API to embed it anywhere. Built
                in Rust for performance and portability.
              </p>
            }
          />
          <FeatureCard
            icon="🔌"
            title="Extensible"
            description={
              <p>
                Connect to 70+ extensions — databases, APIs, browsers, GitHub,
                Google Drive, and more — via the{" "}
                <a href="https://modelcontextprotocol.io/" target="_blank" rel="noopener">
                  Model Context Protocol
                </a>{" "}
                open standard. Add community{" "}
                <Link to="/skills">skills</Link>, or{" "}
                <Link to="/docs/tutorials/custom-extensions">build your own</Link>.
              </p>
            }
          />
          <FeatureCard
            icon="🤖"
            title="Any LLM, including your subscriptions"
            description={
              <p>
                Works with 15+ providers — Anthropic, OpenAI, Google, Ollama,
                OpenRouter, Azure, Bedrock, and more. Use API keys or your
                existing Claude, ChatGPT, or Gemini subscriptions via{" "}
                <Link to="/docs/guides/acp-providers">ACP</Link>.
              </p>
            }
          />
        </div>
        <div className={styles.featuresGridBottom}>
          <SmallCard
            icon="📋"
            title="Recipes"
            description={
              <p>
                Capture workflows as portable YAML configs. Share with your
                team, run in CI, include instructions, extensions, parameters,
                and{" "}
                <Link to="/docs/guides/recipes/session-recipes">subrecipes</Link>.
              </p>
            }
          />
          <SmallCard
            icon="🧩"
            title="MCP Apps"
            description={
              <p>
                Extensions can render interactive UIs directly inside Ponduin
                Desktop — buttons, forms, visualizations. A new way to build{" "}
                <Link to="/docs/tutorials/building-mcp-apps">
                  agent-powered tools
                </Link>.
              </p>
            }
          />
          <SmallCard
            icon="🔀"
            title="Subagents"
            description={
              <p>
                Spawn independent{" "}
                <Link to="/docs/guides/context-engineering/subagents">subagents</Link> to handle
                tasks in parallel — code review, research, file processing —
                keeping the main conversation clean.
              </p>
            }
          />
          <SmallCard
            icon="🔒"
            title="Security"
            description={
              <p>
                Prompt injection detection, tool permission controls, sandbox
                mode, and an{" "}
                <Link to="/docs/guides/security/adversary-mode">
                  adversary reviewer
                </Link>{" "}
                that watches for unsafe actions.
              </p>
            }
          />
        </div>
      </div>
    </section>
  );
}

function StandardsSection() {
  return (
    <section className={`${styles.section} ${styles.sectionAlt}`}>
      <div className={styles.container}>
        <h2 className={styles.sectionTitle}>Built on open standards</h2>
        <div className={styles.standardsGrid}>
          <div className={styles.standardCard}>
            <h3>Model Context Protocol</h3>
            <p>
              <a href="https://modelcontextprotocol.io/" target="_blank" rel="noopener">MCP</a>{" "}
              is the open standard for connecting AI agents to tools and data
              sources. Ponduin connects local workflows to compatible tools and
              data sources through a precise, controlled integration layer.
            </p>
            <Link to="/docs/category/mcp-servers">Browse MCP extensions →</Link>
          </div>
          <div className={styles.standardCard}>
            <h3>Agent Client Protocol</h3>
            <p>
              <a href="https://agentclientprotocol.com/" target="_blank" rel="noopener">ACP</a>{" "}
              is a standard for communicating with coding agents. Ponduin works as
              an ACP server — connect from Zed, JetBrains, or VS Code — and can
              use ACP agents like Claude Code and Codex as providers.
            </p>
            <Link to="/docs/guides/acp-clients">Ponduin as ACP server →</Link>
          </div>
          <div className={styles.standardCard}>
            <h3>PondSec</h3>
            <p>
              Ponduin is developed by{" "}
              <a href="https://pondsec.com/" target="_blank" rel="noopener">
                PondSec
              </a>{" "}
              with a focus on privacy, local control and professional automation.
            </p>
            <a href="https://pondsec.com/" target="_blank" rel="noopener">
              Learn about PondSec →
            </a>
          </div>
        </div>
      </div>
    </section>
  );
}

function EcosystemSection() {
  return (
    <section className={styles.section}>
      <div className={styles.container}>
        <h2 className={styles.sectionTitle}>Ponduin ecosystem</h2>
        <p className={styles.sectionSubtitle}>
          Documentation, integrations and product guidance for reliable local
          agent workflows.
        </p>
        <div className={styles.communityGrid}>
          <a
            href="https://pondsec.com"
            target="_blank"
            rel="noopener"
            className={styles.communityCard}
          >
            <h3>🏢 PondSec</h3>
            <p>
              Learn more about the team and security focus behind Ponduin.
            </p>
          </a>
          <a
            href="https://ponduin.de/docs"
            target="_blank"
            rel="noopener"
            className={styles.communityCard}
          >
            <h3>📚 Documentation</h3>
            <p>
              Install, configure and operate Ponduin across desktop and terminal.
            </p>
          </a>
          <Link to="/extensions" className={styles.communityCard}>
            <h3>🧩 Extensions</h3>
            <p>Connect compatible MCP tools and extend controlled workflows.</p>
          </Link>
          <Link to="/blog" className={styles.communityCard}>
            <h3>📝 Blog</h3>
            <p>Tutorials, technical deep dives and product release notes.</p>
          </Link>
        </div>
      </div>
    </section>
  );
}

function InstallSection() {
  return (
    <section className={`${styles.section} ${styles.sectionAlt}`}>
      <div className={styles.container}>
        <h2 className={styles.sectionTitle}>Get started</h2>
        <div className={styles.installBlock}>
          <div className={styles.installDesktop}>
            <Link
              className="button button--primary button--lg"
              to="docs/getting-started/installation"
            >
              Download the desktop app
            </Link>
            <p className={styles.installPlatforms}>
              Available for macOS, Linux, and Windows
            </p>
          </div>
          <div className={styles.installDivider}>
            <span>or install the CLI</span>
          </div>
          <div className={styles.installTerminal}>
            <div className={styles.terminalBar}>
              <span className={styles.terminalDot} />
              <span className={styles.terminalDot} />
              <span className={styles.terminalDot} />
            </div>
            <pre className={styles.terminalBody}>
              <code>
{`curl -fsSL https://github.com/PondSec/ponduin/releases/download/stable/download_cli.sh | bash`}
              </code>
            </pre>
          </div>
        </div>
      </div>
    </section>
  );
}

export default function Home(): ReactNode {
  return (
    <Layout description="Ponduin is a privacy-first local AI agent for coding, terminal workflows, file management and automation.">
      <HeroSection />
      <main>
        <FeaturesSection />
        <StandardsSection />
        <EcosystemSection />
        <InstallSection />
      </main>
    </Layout>
  );
}
