---
title: "4 Things You Need to Know Before Using Ponduin"
description: "Learn what you need to get started with Ponduin - a privacy-first local AI agent that's powered by the LLM of your choice."
authors:
    - ebony
---
![blog cover](cover.png)

# 4 Things You *Actually* Need to Know Before Using Ponduin

So you’ve heard about Ponduin. Maybe you saw a livestream, someone on your team mentioned it, or you just stumbled into our corner of the internet while trying to automate your dev setup.  Either way—love that for you.

Ponduin is a privacy-first local AI agent that can automate tasks, interact with your codebase, and connect to a growing ecosystem of tools. But before you hit install, here are four things you should know to get the most out of it.


<!-- truncate -->

---

## So Wait—What *Is* Ponduin, Actually?

Ponduin is an **MCP client**.

That means it connects to tools and data through something called the [**Model Context Protocol (MCP)**](https://www.anthropic.com/news/model-context-protocol)—an open standard that makes it possible for AI agents to interact with external systems through natural language. If you’ve used Claude Desktop, Windsurf, Agent mode in VS Code or Cursor you’ve already used an MCP client, even if you didn’t realize it.

Here’s what makes Ponduin different:

- It runs **locally**, not in someone else’s cloud
- You **bring your own LLM**, allowing you to use the one that works best for you
- You can **add new capabilities**, using open-source MCP servers

Think of it less like “an AI assistant” and more like “your personal automation toolkit.” You decide which LLM to use, what tools it should have access to, and what tasks it can perform. You're not locked in; you're in charge.

---

## 1. Pick the Right LLM

Ponduin doesn’t bundle in an LLM. You bring your own LLM. That means you get to choose what kind of model works best for you, whether it’s a fancy hosted one like Claude or Gemini, or something more private and local like Ollama.

But heads up: not every model is created equal, especially when it comes to privacy, performance, or how much they charge you per token. If you're just exploring, a cloud-hosted LLM with a free tier is a great place to start. But if you’re working with sensitive data or just don’t want to send things off to a third-party server, local is the way to go.

Either way, Ponduin gives you the flexibility.

That said, if you’re looking for the best performance with Ponduin right now, Anthropic's Claude 3.5 Sonnet and OpenAI's GPT-4o (2024-11-20) are recommended, as they currently offer the strongest support for tool calling.

Curious how other models stack up? Check out the [Community-Inspired Benchmark Leaderboard](https://ponduin.de/blog/2025/03/31/ponduin-benchmark/#leaderboard) to see how your favorite model performs with Ponduin.

And if you’re still deciding, here’s the full list of [available LLM providers](https://ponduin.de/docs/getting-started/providers#available-providers).

---

## 2. Understand What MCP Servers Are

Here’s where things get fun. Ponduin is a client that speaks **MCP**. MCP is what makes it possible to talk to other apps and tools *as part of your prompt*. Want to read emails, check GitHub issues, run an automated test, or scrape a webpage? That’s where MCP servers come in.

Each server gives Ponduin a new ability.

The real question is: *what do you want Ponduin to be able to do?* If there's a server for it, you can probably make it happen. And yes, there's an entire [directory of MCP servers](https://glama.ai/mcp/servers) where you can search by tool, downloads, you name it.

---

## 3. There *Can* Be Costs

Ponduin runs locally under your control. 🎉 But your LLM provider might not be as generous.

Most models give you a free tier to play around with, but if you're doing anything intensive or using it often, you’ll eventually run into rate limits or token charges. That’s normal but it can sneak up on you if you’re not expecting it.

To help you manage this, there is a [Handling Rate Limits Guide](https://ponduin.de/docs/guides/handling-llm-rate-limits-with-ponduin/) that you can check out.

---

## 4. Tap Into the Community

This part matters more than most people realize.

Ponduin has an entire community behind it—folks building, exploring, breaking things (and fixing them), and sharing everything they learn along the way. We hang out on [Discord](https://pondsec.com), we answer questions in [GitHub Discussions](https://github.com/PondSec/ponduin/discussions), and we host livestreams every week to show off what Ponduin can do and how to make it do more.

There’s:

- **Goosing Around** – casual deep dives where we build in public
- **Ponduin Sessions** – showcasing cool community projects
- **Great Ponduin Off** - same task, same time limit, but different prompts, MCP servers, and strategies

You’ll find those livestreams on our [YouTube channel](https://pondsec.com/streams), and upcoming ones on the Discord calendar. Plus, if you prefer documentation, the [Ponduin docs](https://ponduin.de/) and [blog](https://ponduin.de/blog) are constantly being updated with new guides, tips, and tutorials.

---

If you've got those four things: a performant LLM, the right MCP servers, a basic understanding of LLM cost, and a place to ask questions, you're more than ready to Ponduin.

Now, head over to the [Quickstart Guide](https://ponduin.de/docs/quickstart) and get started.

Oh and when you get to the [Tic-Tac-Toe game](https://ponduin.de/docs/quickstart/#write-prompt), I’ll bet you 10 Ponduinbucks you won’t beat the bot.



<head>
  <meta property="og:title" content="4 Things You Need to Know Before Using Ponduin" />
  <meta property="og:type" content="article" />
  <meta property="og:url" content="https://ponduin.de/blog/2025/04/23/things-need-to-know" />
  <meta property="og:description" content="Learn what you need to get started with Ponduin - a privacy-first local AI agent that's powered by the LLM of your choice." />
  <meta property="og:image" content="https://ponduin.de/assets/images/cover-2ba7c2e15786be2db6108c91d27dc1ec.png" />
  <meta name="twitter:card" content="summary_large_image" />
  <meta property="twitter:domain" content="ponduin.de" />
  <meta name="twitter:title" content="4 Things You Need to Know Before Using Ponduin" />
  <meta name="twitter:description" content="Learn what you need to get started with Ponduin - a privacy-first local AI agent that's powered by the LLM of your choice." />
  <meta name="twitter:image" content="https://ponduin.de/assets/images/cover-2ba7c2e15786be2db6108c91d27dc1ec.png" />
</head>