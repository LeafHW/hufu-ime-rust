import { useEffect, useState } from 'react';

/** 跟随 body.light 的响应式浅色状态（主题切换、系统配色变化都会更新）。 */
export function useBodyLight(): boolean {
  const [light, setLight] = useState(
    typeof document !== 'undefined' && document.body.classList.contains('light'),
  );
  useEffect(() => {
    const sync = () => setLight(document.body.classList.contains('light'));
    sync();
    const mo = new MutationObserver(sync);
    mo.observe(document.body, { attributes: true, attributeFilter: ['class'] });
    const mq = window.matchMedia('(prefers-color-scheme: light)');
    mq.addEventListener?.('change', sync);
    return () => {
      mo.disconnect();
      mq.removeEventListener?.('change', sync);
    };
  }, []);
  return light;
}

/** 当前屏是否是 route：所有页面常驻挂载，隐藏页的轮询/演示要主动停。 */
export function useScreenActive(route: string): boolean {
  const [active, setActive] = useState(
    typeof document !== 'undefined' && document.body.dataset.screen === route,
  );
  useEffect(() => {
    const sync = () => setActive(document.body.dataset.screen === route);
    sync();
    const mo = new MutationObserver(sync);
    mo.observe(document.body, { attributes: true, attributeFilter: ['data-screen'] });
    return () => mo.disconnect();
  }, [route]);
  return active;
}
