# Security Policy

## Supported Versions

| Version | Supported          |
| ------- | ------------------ |
| 0.1.x   | :white_check_mark: |

## Reporting a Vulnerability

We take security vulnerabilities seriously. If you discover a security issue, please report it responsibly.

### How to Report

**Please do NOT report security vulnerabilities through public GitHub issues.**

Instead, please send an email to: **hello@syncmyorders.com**

Include the following information:

- Type of vulnerability (e.g., SQL injection, buffer overflow, privilege escalation)
- Location of the affected source code (file path, line numbers)
- Steps to reproduce the issue
- Potential impact of the vulnerability
- Any suggested fixes (if available)

### What to Expect

1. **Acknowledgment**: We will acknowledge receipt of your report within 48 hours
2. **Assessment**: We will investigate and assess the vulnerability within 7 days
3. **Updates**: We will keep you informed of our progress
4. **Resolution**: We aim to release a fix within 30 days for critical issues
5. **Credit**: We will credit you in the security advisory (unless you prefer anonymity)

### Disclosure Policy

- Please give us reasonable time to address the issue before public disclosure
- We follow coordinated disclosure practices
- Security advisories will be published via GitHub Security Advisories

## Security Best Practices

When deploying Runtara:

### Database Security

- Use strong, unique passwords for PostgreSQL
- Restrict database access to necessary hosts only
- Enable SSL/TLS for database connections in production

### Network Security

- Run runtara-core and runtara-environment behind a firewall
- Use TLS certificates for QUIC connections in production
- Do not expose management ports (8002, 8003) to the public internet

### Configuration

- Never set `RUNTARA_SKIP_CERT_VERIFICATION=true` in production
- Review and restrict container capabilities when using OCI runner
- Use separate database credentials for different environments

## Known Security Considerations

### AGPL License

This software is licensed under AGPL-3.0-or-later. If you modify and deploy this software as a network service, you must make your modifications available under the same license.

### Multi-tenancy

Runtara provides tenant isolation via `tenant_id`. Ensure your product layer properly authenticates and authorizes tenant access.

## Contact

For security-related inquiries: hello@syncmyorders.com

For general questions: Open a GitHub issue
