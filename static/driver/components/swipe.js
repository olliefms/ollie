export function attachHorizontalSwipe(el, { onPrev, onNext, canPrev, canNext }) {
  let startX = 0, startY = 0, tracking = false;
  const THRESHOLD = 60;

  el.addEventListener('pointerdown', e => {
    if (e.pointerType === 'mouse') return;
    startX = e.clientX; startY = e.clientY; tracking = true;
  });
  el.addEventListener('pointerup', e => {
    if (!tracking) return;
    tracking = false;
    const dx = e.clientX - startX;
    const dy = e.clientY - startY;
    if (Math.abs(dx) < THRESHOLD) return;
    if (Math.abs(dx) <= Math.abs(dy) * 1.5) return;
    if (dx > 0 && canPrev && canPrev()) onPrev();
    else if (dx < 0 && canNext && canNext()) onNext();
  });
  el.addEventListener('pointercancel', () => { tracking = false; });
}
