<!-- TODO: confirm Runtara fork URL and contact email before public release. -->

# Contributing to Runtara Frontend

Thank you for your interest in contributing! This document provides guidelines and instructions for contributing.

## Contributor License Agreement (CLA)

**All contributions require signing our [Contributor License Agreement (CLA)](CLA.md).** We cannot accept pull requests from contributors who have not signed the CLA.

Before your first contribution, please read the CLA and include the following statement in your pull request:

```
I have read the Runtara Contributor License Agreement and I agree to its terms.

Name: [Your Full Name]
Email: [Your Email]
Date: [Date]
GitHub Username: [Your GitHub Username]
```

If you have questions about the CLA, contact us at legal@syncmyorders.com.

## Code of Conduct

This project adheres to a [Code of Conduct](CODE_OF_CONDUCT.md). By participating, you are expected to uphold this code.

## How to Contribute

### Reporting Bugs

Before creating a bug report, please check existing issues to avoid duplicates. When creating a bug report, include:

- A clear, descriptive title
- Steps to reproduce the issue
- Expected vs actual behavior
- Your environment (OS, browser, Node.js version)
- Relevant screenshots or error messages

### Suggesting Features

Feature requests are welcome! Please:

- Check existing issues and discussions first
- Describe the problem your feature would solve
- Explain how you envision the solution
- Consider if it fits the project's scope

### Pull Requests

1. **Sign the CLA** if this is your first contribution (see above)
2. **Fork the repository** and create your branch from `main`
3. **Follow the coding standards** (see below)
4. **Add tests** for new functionality
5. **Ensure all tests pass** locally
6. **Update documentation** if needed
7. **Submit your PR** with a clear description

## Development Setup

### Prerequisites

- Node.js 22+
- npm

### Building

```bash
# Clone your fork
git clone https://github.com/YOUR_USERNAME/syncmyorders-frontend.git
cd syncmyorders-frontend

# Install dependencies
npm ci

# Copy environment variables
cp .env.example .env
# Edit .env with your configuration

# Start development server
npm run dev
```

### Running Tests

```bash
# Run tests once
npm test

# Run tests in watch mode
npm run test:watch

# Run tests with coverage
npm run test:coverage
```

## Coding Standards

### Formatting & Linting

```bash
npm run lint
```

### Commit Messages

- Use clear, descriptive commit messages
- Start with a type prefix (e.g., "fix:", "feat:", "chore:")
- Reference issues when applicable (e.g., "Fix #123")

### Code Style

- Follow existing TypeScript patterns in the codebase
- Use functional React components with hooks
- Prefer descriptive names over comments
- Keep components focused and reasonably sized

### Testing

- Write tests for new functionality
- Tests should be deterministic and fast
- Co-locate tests with source files using `.test.ts(x)` extension

## License

By contributing, you agree that your contributions will be licensed under the AGPL-3.0-or-later license.

## Questions?

Feel free to open an issue for questions about contributing. We're happy to help!
