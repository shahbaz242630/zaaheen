# Security Policy

Zaaheen holds what people choose to tell their AI assistants about themselves: their work, their health, their family. We take every report about it seriously.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.** Report it privately, in either of these ways:

- **[Open a private security advisory](https://github.com/shahbaz242630/zaaheen/security/advisories/new)** on GitHub. Only we can see it until an advisory is published, and we can work through the fix with you there.
- **Email [customerservice@zaaheen.com](mailto:customerservice@zaaheen.com)** with "Security" in the subject.

Please include what you found, how to reproduce it, the version of Zaaheen you were using, and what you think the impact is. A proof of concept helps, but a clear description is welcome on its own.

## What to expect

- We reply within **5 working days**, and keep you updated until the problem is fixed.
- We tell you honestly whether we agree it is a vulnerability, and why.
- We thank you publicly when the fix ships, unless you would rather stay anonymous.
- We do not run a paid bug bounty at the moment.

## Rules for good-faith research

Please give us **90 days** to fix a problem before telling anyone else, do not access or change other people's data, and do not disrupt our services. Test only with accounts and data that are your own.

If you follow these rules, we will not take legal action against you for your research.

## What is in scope

The promise Zaaheen is built on:

> **Your memories stay encrypted on your own computer. Our servers cannot read them.**

Anything that breaks that promise, or exposes someone's memories to anyone who should not have them, is in scope. We especially want to hear about:

- **Memories readable on disk.** Any memory data written to disk unencrypted, or any way around the encryption.
- **Key handling.** The encryption key leaking into logs, crash reports, temporary files, or staying in memory longer than it needs to.
- **Instructions hidden in memories.** Stored text that makes an AI app act on it as an instruction instead of treating it as information.
- **Deleting that does not delete.** "Delete everything" leaving readable data behind.
- **Our account service and websites.** zaaheen.com, account.zaaheen.com and the service behind sign-in and subscriptions, for example seeing or changing another person's account or subscription.
- **Supply chain.** A dependency, build step, or download that is not what it claims to be.

## Out of scope

- **Scanner results with no demonstrated impact.** Tell us what an attacker could actually do.
- **Attacks that need an already-compromised computer.** Someone who is already signed in to your Windows account, with access to its saved passwords and keys, is inside the protection by design.
- **The Windows installer is not code-signed yet.** This is a known, deliberate choice for now. Windows may show an "unknown publisher" warning; always download Zaaheen from zaaheen.com.
- **Slowing down or crashing your own copy of the app** with your own data.
- **Problems in services run by our sign-in or payment providers.** Please report those to the provider directly.

## Supported versions

Only the latest version of Zaaheen from [zaaheen.com](https://zaaheen.com) is supported. Security fixes go into the next release; older versions do not get separate fixes.

## How we look after the code

Every change to this repository runs through:

- Secret scanning with push protection, and regular scans of the full history
- Checks on every dependency for known vulnerabilities, licences and where it comes from
- Static analysis with CodeQL
- Encryption and hidden-instruction tests that must pass before anything is merged
- Build tools pinned to exact, verified versions

Thank you for helping keep people's memories private.
