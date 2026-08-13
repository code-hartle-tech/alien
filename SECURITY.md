# Security policy

Alien brokers privileged firmware and GPU operations. Treat vulnerabilities in
the daemon, socket policy, package permissions, payload validation, rollback,
or model guards as security-sensitive.

Please report vulnerabilities privately to
[security@hartle.tech](mailto:security@hartle.tech) before opening a public
issue. Include the Alien version, distribution, model, BIOS, transport in use,
and the smallest safe reproduction you can provide.

Examples in scope include:

- bypasses of the daemon's operation or payload allowlist;
- unsafe socket ownership, permissions, framing, timeout or rate limiting;
- model/device guard bypasses around firmware or NVML mutations;
- partial transactions that evade readback or rollback;
- package/service permissions that grant more authority than documented.

Do not send proprietary Acer installers, firmware images, credentials, or
personal machine data. Hashes, public Acer download URLs, minimized payloads,
and redacted logs are sufficient.

The currently supported release line is 0.5.x. Coordinated fixes are published
to the public GitHub repository after the agreed disclosure window.

Please allow a reasonable coordinated-disclosure window. We will acknowledge
receipt, reproduce without expanding the unsafe surface, and credit the
reporter unless anonymity is requested.
