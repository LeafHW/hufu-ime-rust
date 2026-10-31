import { PageHead } from '../components/ui';

export function DonatePage() {
  return (
    <>
      <PageHead title="打赏支持" sub="虎符输入法由作者业余时间开发维护，免费开源" />
      <div className="donate-page card">
        <div className="donate-qr">
          <img src="/donate.png" alt="打赏码" />
        </div>
        <div className="donate-text">
          <h3>扫码打赏，支持作者继续开发</h3>
          <p>
            如果虎符好用，欢迎扫码请作者喝杯咖啡。每一份支持都会变成继续迭代的动力——
            整句模型、云同步、皮肤体系，都因你们的支持而持续进化。
          </p>
          <p className="donate-note">金额随意，心意最重要。谢谢！</p>
        </div>
      </div>
    </>
  );
}
