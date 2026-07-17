// Stylised brand banner printed to the browser console on startup. The "O"
// is a ring that crosses three chunky triple-nodes arranged as an upside-down
// triangle (joined by straight edges); then the block-letter "Open" /
// "Triplestore" wordmark. A compact mark is used on narrow viewports.
// Mirrors the banner the backend prints.

export function logBanner(): void {
  const full = [
    '',
    '       ▄▄▄▄▄▄▄▄▄▄▄',
    ' ▄██▄█▀▀         ▀▀█▄██▄',
    ' ████───────────────████',
    ' ▀██▀               ▀██▀',
    ' █▀ ╲               ╱ ▀█    ████▄ ▄█▀█▄ ████▄',
    ' █    ╲           ╱    █    ██ ██ ██▄█▀ ██ ██',
    ' █▄    ╲         ╱    ▄█    ████▀ ▀█▄▄▄ ██ ██',
    '  █     ╲       ╱     █     ██',
    '  ▀█▄    ╲ ▄██▄╱    ▄█▀     ▀▀',
    '    ▀█▄▄   ████  ▄▄█▀',
    '       ▀▀▀▀▀██▀▀▀▀',
    '      ▄▄▄▄▄▄▄▄▄              ▄▄',
    '      ▀▀▀███▀▀▀    ▀▀        ██              ██',
    '         ███ ████▄ ██  ████▄ ██ ▄█▀█▄ ▄█▀▀▀ ▀██▀▀ ▄███▄ ████▄ ▄█▀█▄',
    '         ███ ██ ▀▀ ██  ██ ██ ██ ██▄█▀ ▀███▄  ██   ██ ██ ██ ▀▀ ██▄█▀',
    '         ███ ██    ██▄ ████▀ ██ ▀█▄▄▄ ▄▄▄█▀  ██   ▀███▀ ██    ▀█▄▄▄',
    '                       ██',
    '                       ▀▀',
  ];
  const compact = [
    '',
    '      ▄▄▄▄▄▄▄▄▄',
    ' ▄██▄█▀       ▀█▄██▄',
    ' ████───────────████',
    ' ▀██▀           ▀██▀   OpenTriplestore',
    ' █  ╲           ╱  █   RDF triple store',
    ' █   ╲         ╱   █',
    ' █    ╲       ╱    █',
    ' ▀█    ╲     ╱    █▀',
    '  ▀█▄   ╲▄██▄   ▄█▀',
    '    ▀█▄  ████ ▄█▀',
    '      ▀▀▀▀██▀▀▀',
  ];
  // The console's character width is not directly knowable; approximate it
  // from the viewport so narrow windows get the compact mark.
  const cols = Math.floor((typeof window !== 'undefined' ? window.innerWidth : 1024) / 8);
  const wide = cols >= 68;
  const mark = (wide ? full : compact).join('\n');
  const tagline = wide
    ? '  A modern RDF triple store · SPARQL 1.1/1.2 · GeoSPARQL\n'
    : '  SPARQL 1.1/1.2 · GeoSPARQL\n';
  // Intentional: this is the styled startup banner printed to the dev console.
  // eslint-disable-next-line no-console
  console.log(
    `%c${mark}\n%c${tagline}`,
    // line-height:1 so the half-blocks tile seamlessly into solid shapes.
    'color:#7ED6D0; font-family:monospace; font-weight:400; line-height:1;',
    'color:#8fb3bb; font-family:monospace; font-size:11px;'
  );
}
