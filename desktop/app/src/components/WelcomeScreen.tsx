interface WelcomeScreenProps {
  step: number;
  onBack: () => void;
  onNext: () => void;
  onFinish: () => void;
  onSkip: () => void;
  onOpenSettings?: () => void;
}

const STEPS = [
  {
    eyebrow: 'Welcome to Intent',
    title: 'Your work stays understandable.',
    body: 'Intent turns the activity you choose to capture into a local timeline, so you can find yesterday’s work without relying on memory.',
    details: ['Your timeline is stored locally.', 'You can use it without a cloud provider.', 'Capture controls stay under your account.'],
  },
  {
    eyebrow: 'Restore with context',
    title: 'Review first. Continue when you are ready.',
    body: 'Review opens the saved restore context alongside your sessions. Resume reopens the saved context; Continue opens only that session’s saved Firefox tabs.',
    details: ['Review shows exactly what can reopen.', 'Resume never runs the last shell command.', 'Continue does not open files or a terminal.'],
  },
  {
    eyebrow: 'Optional Copilot',
    title: 'Connect a provider only if you want one.',
    body: 'Copilot is off by default. In Settings you can choose an LLM provider and store its API key locally for your user account.',
    details: ['The key is never displayed after saving.', 'You can switch providers later.', 'Local timeline and restore work without a key.'],
  },
];

export function WelcomeScreen({ step, onBack, onNext, onFinish, onSkip, onOpenSettings }: WelcomeScreenProps) {
  const current = STEPS[step] ?? STEPS[0];
  const lastStep = step === STEPS.length - 1;
  return (
    <section className="welcome-screen" role="dialog" aria-modal="true" aria-labelledby="welcome-title">
      <div className="welcome-progress" aria-label={'Welcome step ' + (step + 1) + ' of ' + STEPS.length}>
        {STEPS.map((item, index) => <span className={index === step ? 'is-current' : index < step ? 'is-complete' : ''} key={item.title} />)}
      </div>
      <span className="section-kicker">{current.eyebrow}</span>
      <h1 id="welcome-title">{current.title}</h1>
      <p>{current.body}</p>
      <ul>
        {current.details.map((detail) => <li key={detail}>{detail}</li>)}
      </ul>
      <div className="welcome-actions">
        <button className="compact-quiet" type="button" onClick={onSkip}>Skip for now</button>
        <div>
          {step > 0 && <button className="compact-quiet" type="button" onClick={onBack}>Back</button>}
          {lastStep && onOpenSettings && <button className="compact-quiet" type="button" onClick={onOpenSettings}>Set up Copilot</button>}
          <button className="compact-primary" type="button" onClick={lastStep ? onFinish : onNext}>{lastStep ? 'Open Intent' : 'Next'}</button>
        </div>
      </div>
    </section>
  );
}
