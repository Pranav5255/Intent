import { useEffect, useRef, useState } from 'react';
import { Icon, type IconName } from '../components/Icon';
import { Keys } from '../components/ui';
import { SettingsQuery } from './kit';
import { AboutSection, CaptureSection, GeneralSection, PrivacySection, ShortcutsSection, StorageSection } from './sections';
import { IntelligenceSection } from './Intelligence';

const SECTIONS: Array<{ id: string; title: string; icon: IconName }> = [
  { id: 'general', title: 'General', icon: 'sliders' },
  { id: 'capture', title: 'Capture', icon: 'capture' },
  { id: 'privacy', title: 'Privacy', icon: 'shield' },
  { id: 'intelligence', title: 'Intelligence', icon: 'chip' },
  { id: 'storage', title: 'Storage', icon: 'database' },
  { id: 'shortcuts', title: 'Shortcuts', icon: 'keyboard' },
  { id: 'about', title: 'About', icon: 'info' },
];

export function SettingsView({ section, onSectionShown }: { section: string | null; onSectionShown: () => void }) {
  const [q, setQ] = useState('');
  const [active, setActive] = useState(section ?? 'general');
  const scroller = useRef<HTMLDivElement>(null);
  const searchRef = useRef<HTMLInputElement>(null);

  const jump = (id: string, smooth = true) => {
    const el = scroller.current?.querySelector<HTMLElement>(`#settings-${id}`);
    if (el && scroller.current) {
      const reduce = window.matchMedia('(prefers-reduced-motion: reduce)').matches;
      scroller.current.scrollTo({ top: el.offsetTop - 24, behavior: smooth && !reduce ? 'smooth' : 'auto' });
    }
    setActive(id);
  };

  useEffect(() => {
    if (!section) return;
    // Wait a frame so async sections have laid out.
    const t = window.setTimeout(() => {
      jump(section, false);
      onSectionShown();
    }, 60);
    return () => window.clearTimeout(t);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [section]);

  // Track which section is in view for the inner nav.
  useEffect(() => {
    const root = scroller.current;
    if (!root) return;
    const onScroll = () => {
      const sections = [...root.querySelectorAll<HTMLElement>('[data-section]')];
      const top = root.scrollTop + 80;
      let current = sections[0]?.dataset.section ?? 'general';
      for (const s of sections) if (s.offsetTop <= top) current = s.dataset.section ?? current;
      if (root.scrollTop + root.clientHeight >= root.scrollHeight - 4) current = sections[sections.length - 1]?.dataset.section ?? current;
      setActive(current);
    };
    root.addEventListener('scroll', onScroll, { passive: true });
    return () => root.removeEventListener('scroll', onScroll);
  }, []);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === 'f') {
        e.preventDefault();
        searchRef.current?.focus();
      }
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, []);

  return (
    <div className="set">
      <aside className="set-nav">
        <div className="set-search">
          <Icon name="search" size={14} />
          <input
            ref={searchRef}
            placeholder="Search settings"
            value={q}
            onChange={(e) => setQ(e.target.value)}
            onKeyDown={(e) => e.key === 'Escape' && setQ('')}
            aria-label="Search settings"
          />
          {!q && <Keys keys={['mod', 'F']} />}
        </div>
        <div className="set-nav-list">
          {SECTIONS.map((s) => (
            <button key={s.id} className={`set-nav-item ${active === s.id && !q ? 'is-active' : ''}`} onClick={() => jump(s.id)}>
              <Icon name={s.icon} size={15} />
              {s.title}
            </button>
          ))}
        </div>
      </aside>
      <div className={`set-scroll ${q ? 'is-searching' : ''}`} ref={scroller}>
        <SettingsQuery.Provider value={q}>
          <div className="set-content">
            <GeneralSection />
            <CaptureSection />
            <PrivacySection />
            <IntelligenceSection />
            <StorageSection />
            <ShortcutsSection />
            <AboutSection />
            <div className="set-noresults">
              <Icon name="search" size={22} />
              <div>No settings match “{q}”</div>
            </div>
          </div>
        </SettingsQuery.Provider>
      </div>
    </div>
  );
}
