const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
const finePointer = matchMedia('(pointer: fine)');
const targets = document.querySelectorAll('.hero-copy, .network-art, .workspace-heading, .workspace-aside, .console, .section-head, .launch-card, .manifesto-item, .policy-card, .repo-card, .evidence-link, .helper-card, .quick-guide > div, .recovery-note, .home-close');

if (!reducedMotion.matches && 'IntersectionObserver' in window) {
  document.documentElement.classList.add('js-motion');
  const observer = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      if (!entry.isIntersecting) continue;
      entry.target.classList.add('in-view');
      observer.unobserve(entry.target);
    }
  }, { threshold: .08 });
  targets.forEach((element, index) => {
    element.classList.add('reveal');
    element.style.setProperty('--reveal-delay', `${index % 3 * 65}ms`);
    observer.observe(element);
  });
  reducedMotion.addEventListener('change', () => {
    if (!reducedMotion.matches) return;
    targets.forEach((element) => element.classList.add('in-view'));
    observer.disconnect();
  });
}

let pointerFrame;
window.addEventListener('pointermove', (event) => {
  if (reducedMotion.matches || !finePointer.matches || pointerFrame) return;
  pointerFrame = requestAnimationFrame(() => {
    document.documentElement.style.setProperty('--pointer-x', `${event.clientX}px`);
    document.documentElement.style.setProperty('--pointer-y', `${event.clientY}px`);
    pointerFrame = undefined;
  });
}, { passive: true });

for (const card of document.querySelectorAll('.tilt-card')) {
  card.addEventListener('pointermove', (event) => {
    if (reducedMotion.matches || !finePointer.matches) return;
    const rect = card.getBoundingClientRect();
    card.style.setProperty('--tilt-x', `${((event.clientX - rect.left) / rect.width - .5) * 5}deg`);
    card.style.setProperty('--tilt-y', `${((event.clientY - rect.top) / rect.height - .5) * -5}deg`);
  });
  card.addEventListener('pointerleave', () => {
    card.style.removeProperty('--tilt-x');
    card.style.removeProperty('--tilt-y');
  });
}

for (const group of document.querySelectorAll('[role="tablist"], [role="radiogroup"]')) {
  const controls = [...group.querySelectorAll('button')];
  const syncFocus = () => controls.forEach((control) => {
    const selected = control.getAttribute('aria-selected') === 'true' || control.getAttribute('aria-checked') === 'true';
    control.tabIndex = selected ? 0 : -1;
  });
  syncFocus();
  group.addEventListener('click', () => queueMicrotask(syncFocus));
  group.addEventListener('keydown', (event) => {
    const current = controls.indexOf(document.activeElement);
    if (current < 0 || !['ArrowLeft', 'ArrowRight', 'ArrowUp', 'ArrowDown', 'Home', 'End'].includes(event.key)) return;
    event.preventDefault();
    const next = event.key === 'Home' ? 0 : event.key === 'End' ? controls.length - 1 : (current + (['ArrowRight', 'ArrowDown'].includes(event.key) ? 1 : -1) + controls.length) % controls.length;
    controls[next].click();
    controls[next].focus();
    syncFocus();
  });
}

for (const link of document.querySelectorAll('a[href]')) {
  link.addEventListener('click', (event) => {
    if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0 || link.target === '_blank') return;
    const target = new URL(link.href);
    if (target.origin === location.origin && target.pathname !== location.pathname) document.body.dataset.navigating = 'true';
  });
}
window.addEventListener('pageshow', () => { delete document.body.dataset.navigating; });
