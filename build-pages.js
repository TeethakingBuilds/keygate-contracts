import { readFile, writeFile } from 'node:fs/promises';

const source = await readFile(new URL('./site-template.html', import.meta.url), 'utf8');
const styles = source.match(/<style>([\s\S]*?)<\/style>/)[1];
const sections = [...source.matchAll(/<section\b[^>]*>[\s\S]*?<\/section>/g)].map(([html]) => html);
const section = (name) => sections.find((html) => new RegExp(`(?:id="${name}"|aria-labelledby="${name}-title")`).test(html));
const payment = source.slice(source.indexOf('    <div class="console" id="pay"'), source.indexOf('\n  </div>\n</header>'));
const review = source.match(/<dialog\b[\s\S]*?<\/dialog>/)[0];
const brand = source.match(/<a class="brand"[\s\S]*?<\/a>/)[0].replace('href="#top"', 'href="./index.html"');
const footer = source.match(/<footer>[\s\S]*?<\/footer>/)[0].replaceAll('href="#top"', 'href="./index.html"');
const heroCopy = source.match(/<div class="hero-copy">[\s\S]*?\n    <\/div>/)[0].replaceAll('href="#pay"', 'href="./send.html"');
const strip = source.match(/<div class="signal-strip"[\s\S]*?\n<\/div>/)[0];

const arrow = '<svg viewBox="0 0 24 24" fill="none" aria-hidden="true"><path d="M5 12h14M12 5l7 7-7 7" stroke="currentColor" stroke-width="1.5"/></svg>';
const orbit = `<div class="network-art" aria-label="Illustrative native USDC route map, not transaction activity">
  <div class="art-caption"><span class="live-dot"></span> NATIVE ASSET. CLEAR PATH.</div>
  <svg class="orbit-map" viewBox="0 0 600 520" fill="none" aria-hidden="true">
    <defs><linearGradient id="route-gradient" x1="100" y1="200" x2="500" y2="350" gradientUnits="userSpaceOnUse"><stop stop-color="#c7ff4a"/><stop offset="1" stop-color="#a997ff"/></linearGradient></defs>
    <circle class="orbit-ring" cx="300" cy="260" r="180"/><circle class="orbit-ring inner" cx="300" cy="260" r="124"/>
    <path class="orbital-track" d="M104 260C155 80 420 100 496 260C435 425 156 410 104 260Z"/>
    <path class="route-line" d="M104 260H300L454 126M300 260L488 300M300 260L426 420"/>
    <circle cx="104" cy="260" r="34" fill="#13161b" stroke="#c7ff4a"/><path d="M90 257l29-14M90 271l29-14" stroke="#c7ff4a" stroke-width="4"/>
    <circle cx="300" cy="260" r="54" fill="#101218" stroke="#c7ff4a"/><path d="M278 237h30v13h-16v20h16v13h-30v-46Zm30 8h15v30h-15" stroke="#c7ff4a" stroke-width="3"/>
    <circle cx="454" cy="126" r="24" fill="#14131d" stroke="#a997ff"/><path d="M454 110l9 16-9 6-9-6 9-16Zm-9 20l9 12 9-12-9 6-9-6Z" fill="#a997ff"/>
    <circle cx="488" cy="300" r="24" fill="#101b1f" stroke="#68dbe8"/><circle cx="488" cy="300" r="12" stroke="#68dbe8" stroke-width="4"/>
    <circle cx="426" cy="420" r="24" fill="#14131d" stroke="#a997ff"/><path d="M414 412h24m-24 8h24m-24 8h24" stroke="#a997ff" stroke-width="4"/>
  </svg>
  <span class="map-label label-stellar">STELLAR <small>SOURCE · DOMAIN 27</small></span>
  <span class="map-label label-keygate">KEYGATE <small>POLICY BOUNDARY</small></span>
  <span class="map-label label-eth">ETHEREUM</span><span class="map-label label-base">BASE</span><span class="map-label label-sol">SOLANA</span>
  <div class="art-foot"><span>ARCHITECTURE, NOT LIVE ACTIVITY</span><span>CIRCLE CCTP V2</span></div>
</div>`;

const pageHeading = (eyebrow, title, copy) => `<header class="workspace-heading shell"><p class="kicker">${eyebrow}</p><h1>${title}</h1><p>${copy}</p></header>`;
const card = (number, title, text, href, link) => `<a class="launch-card tilt-card" href="${href}"><span class="launch-number">${number} / WORKSPACE</span><h3>${title}</h3><p>${text}</p><span class="launch-link">${link}${arrow}</span></a>`;
const launch = `<section class="launch-section shell" aria-labelledby="workspace-title"><div class="section-head"><p class="kicker">Less noise. More control.</p><h2 id="workspace-title">Your next move<br>has its own space.</h2></div><div class="launch-grid">
${card('01', 'Send', 'Native USDC. Exact amounts. Wallet approval before anything leaves your account.', './send.html', 'Open send')}
${card('02', 'Receive', 'Share your public address. Keep transaction references. Verify outcomes against the ledger.', './receive.html', 'Open receive')}
${card('03', 'Recover', 'Query Circle’s real source messages. Keep attestations separate from destination settlement.', './recovery.html', 'Open recovery')}
</div></section>`;

const send = `${pageHeading('01 / SEND WORKSPACE', 'Value moves.<br><span>Keys don’t.</span>', 'Prepare native USDC with a clear route, exact amount, and explicit wallet approval. Stellar Testnet only.')}
<section class="workspace-layout shell" aria-label="Payment workspace">
  <aside class="workspace-aside">
    <div class="aside-label">YOUR TRANSACTION, IN ORDER</div>
    <ol class="progress-track" id="send-progress">
      <li class="active" data-step="connect"><span>01</span><div><b>Connect your wallet</b><small>Verify account and Testnet</small></div></li>
      <li data-step="prepare"><span>02</span><div><b>Prepare the intent</b><small>Choose path, amount, recipient</small></div></li>
      <li data-step="review"><span>03</span><div><b>Review & sign</b><small>Exact XDR, explicit approval</small></div></li>
      <li data-step="confirm"><span>04</span><div><b>Verify on the ledger</b><small>Submitted is not settled</small></div></li>
    </ol>
    <div class="boundary-card"><span class="live-dot"></span><h3>A deliberate boundary.</h3><p>Direct SAC is a separate Testnet path outside Keygate policy. Policy signing and CCTP burns stay disabled until their deployments are verified.</p><a href="./developers.html">Inspect readiness ${arrow}</a></div>
    <a class="workspace-link" href="./receive.html">Already have a transaction hash? ${arrow}</a>
  </aside>
  ${payment}
</section>${review}`;

const receive = `${pageHeading('02 / RECEIVE WORKSPACE', 'Your address.<br><span>Your receipts.</span>', 'One public identity. No custody. Share your Testnet address and look up transaction outcomes directly on Stellar.')}
<div class="workspace-content">${section('activity')}</div>
<section class="shell quick-guide" aria-label="Receiving checklist"><div><span>01</span><h3>Check the network</h3><p>The sender and receiver must both use Stellar Testnet. Mainnet assets do not belong here.</p></div><div><span>02</span><h3>Check the asset</h3><p>Use Circle’s official native USDC. The receiving account needs the correct USDC trustline.</p></div><div><span>03</span><h3>Check the ledger</h3><p>A shared hash is a reference, not proof. Query the ledger before treating a transfer as confirmed.</p></div></section>
<div class="shell page-next"><span>MAKE YOUR NEXT MOVE</span><a href="./send.html">Prepare a payment ${arrow}</a></div>`;

const recovery = `${pageHeading('03 / RECOVERY WORKSPACE', 'Follow the proof.<br><span>Not the promise.</span>', 'A stalled transfer deserves a clear answer. Query Circle Iris Sandbox and distinguish a source burn, an attestation, and a destination mint.')}
<div class="proof-rail shell" aria-label="CCTP verification stages, not transfer status"><span>01 / SOURCE BURN</span><i></i><span>02 / CIRCLE ATTESTATION</span><i></i><span>03 / DESTINATION MINT</span></div>
<div class="workspace-content">${section('recovery')}</div>
<section class="shell recovery-note"><div class="note-symbol">!</div><div><h3>No blind retries.</h3><p>A timeout is unknown, not failure. Keep your source hash, check the ledger, and inspect Circle’s response before initiating another transfer. Claim submission is not enabled in this preview.</p><a href="./receive.html">Look up the source transaction ${arrow}</a></div></section>`;

const developers = `${pageHeading('04 / DEVELOPER WORKSPACE', 'Built in public.<br><span>Proved in code.</span>', 'A polished interface is the opening act. Review the contracts, reproduce the tests, challenge the boundaries, and contribute a real improvement.')}
${section('stack')}${section('evidence')}${section('readiness')}${section('contribute')}`;

const home = `<header class="hero" id="top"><div class="shell hero-grid">${heroCopy}${orbit}</div></header>${strip}${launch}${section('manifesto')}${section('flow')}${section('policy')}${section('faq')}
<section class="home-close shell"><p class="kicker">Open source. Open to scrutiny.</p><h2>Don’t just watch.<br><span>Build with us.</span></h2><a class="button primary" href="./developers.html">Explore the code ${arrow}</a></section>`;

const pages = [
  { file: 'index.html', key: 'home', title: 'Native USDC. Keys stay yours.', content: home, description: 'Keygate: open-source, non-custodial native USDC payment policy and Circle CCTP V2 infrastructure for Stellar.' },
  { file: 'send.html', key: 'send', title: 'Send native USDC', content: send, description: 'Connect Freighter, prepare and review a native USDC Stellar Testnet payment. No backend wallet keys.' },
  { file: 'receive.html', key: 'receive', title: 'Receive & verify', content: receive, description: 'Share your Stellar Testnet public address and verify transaction outcomes directly from the ledger.' },
  { file: 'recovery.html', key: 'recovery', title: 'Circle proof recovery', content: recovery, description: 'Inspect Circle Iris Sandbox source-burn messages and attestations without claiming unverified destination settlement.' },
  { file: 'developers.html', key: 'developers', title: 'Build in public', content: developers, description: 'Explore Keygate contracts, server and SDK repositories, contributor pathways, tests and deployment readiness.' },
];

for (const page of pages) {
  const links = [['home', 'index.html', 'Home'], ['send', 'send.html', 'Send'], ['receive', 'receive.html', 'Receive'], ['recovery', 'recovery.html', 'Recovery'], ['developers', 'developers.html', 'Developers']];
  const navigation = `<nav aria-label="Primary navigation"><div class="shell nav-inner">${brand}<button class="menu-toggle" type="button" aria-expanded="false" aria-controls="nav-links" aria-label="Toggle navigation"><svg width="23" height="23" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7"><path d="M4 7h16M4 12h16M4 17h16"/></svg></button><div class="nav-links" id="nav-links">${links.map(([key, file, label]) => `<a href="./${file}"${page.key === key ? ' aria-current="page"' : ''}>${label}</a>`).join('')}<button class="wallet-button" id="nav-connect" type="button">Connect wallet</button></div></div></nav>`;
  const document = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>${page.title} — Keygate</title><meta name="description" content="${page.description}"><meta name="theme-color" content="#090a0d"><link rel="stylesheet" href="./web.css"><link rel="stylesheet" href="./motion.css"></head>
<body data-page="${page.key}"><a class="skip-link" href="#page-content">Skip to content</a><div class="page-loader" aria-hidden="true"></div><div class="status-bar"><div class="shell"><i aria-hidden="true"></i>Engineering preview · Testnet first · Unaudited</div></div>${navigation}<p id="page-message" class="page-message shell" role="status" aria-live="polite"></p><main id="page-content">${page.content}</main>${footer}<script type="module" src="./site.js"></script><script type="module" src="./index.js"></script></body></html>
`;
  await writeFile(new URL(`./${page.file}`, import.meta.url), document);
}
await writeFile(new URL('./web.css', import.meta.url), styles.trim() + '\n');
console.log(`Built ${pages.length} Keygate pages and shared styles.`);
