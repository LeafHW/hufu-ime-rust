//! 补充语料 Aho-Corasick 自动机（对齐虎爪 SentenceSupplementMatcher）。
//!
//! 与旧的「段文本精确匹配」不同：沿 beam 路径逐字推进，
//! 句子里**任何位置**出现的补充词都加分（跨段也能命中）。
//!
//! 奖励公式（SentenceWeights）：baseline + scale·ln(w/1000)，上限 maximum。

use std::collections::HashMap;

#[derive(Default, Clone)]
struct Node {
    /// 出边：字符 → 节点下标
    transitions: HashMap<char, usize>,
    /// 失败指针
    failure: usize,
    /// 本节点（含失败链传播）的奖励：取自身与失败链上的最大值
    reward: f64,
}

#[derive(Default, Clone)]
pub struct SupplementAutomaton {
    nodes: Vec<Node>,
}

impl SupplementAutomaton {
    pub fn build(entries: &[(String, f64)], baseline: f64, scale: f64, maximum: f64) -> Self {
        let mut nodes: Vec<Node> = vec![Node::default()];
        for (word, weight) in entries {
            let reward = (baseline + scale * ((weight / 1000.0).ln().max(0.0))).min(maximum);
            if word.is_empty() || reward <= 0.0 {
                continue;
            }
            let mut cur = 0usize;
            for ch in word.chars() {
                let next = match nodes[cur].transitions.get(&ch) {
                    Some(&id) => id,
                    None => {
                        let id = nodes.len();
                        nodes.push(Node::default());
                        nodes[cur].transitions.insert(ch, id);
                        id
                    }
                };
                cur = next;
            }
            nodes[cur].reward = nodes[cur].reward.max(reward);
        }
        if nodes.len() <= 1 {
            return SupplementAutomaton { nodes };
        }
        // BFS 建失败指针；失败链上的 reward 传播到节点（一次取 max 即可，
        // 不重复计分——「赢麻了」与「麻了」同时在词包时「了」字只加一次）。
        let mut queue = std::collections::VecDeque::new();
        let roots: Vec<usize> = nodes[0].transitions.values().copied().collect();
        for child in roots {
            nodes[child].failure = 0;
            queue.push_back(child);
        }
        while let Some(idx) = queue.pop_front() {
            let edges: Vec<(char, usize)> = nodes[idx].transitions.iter().map(|(&c, &v)| (c, v)).collect();
            for (ch, child) in edges {
                let mut f = nodes[idx].failure;
                while f != 0 && !nodes[f].transitions.contains_key(&ch) {
                    f = nodes[f].failure;
                }
                let target = nodes[f].transitions.get(&ch).copied().unwrap_or(0);
                nodes[child].failure = if target != child { target } else { 0 };
                let fail_reward = nodes[nodes[child].failure].reward;
                nodes[child].reward = nodes[child].reward.max(fail_reward);
                queue.push_back(child);
            }
        }
        SupplementAutomaton { nodes }
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.len() <= 1
    }

    /// 推进一个字符，返回 (新状态, 本字获得的奖励)。
    /// 词在本次推进完成时（含失败链传播）返回奖励。
    pub fn advance(&self, state: usize, ch: char) -> (usize, f64) {
        if self.is_empty() {
            return (0, 0.0);
        }
        let mut s = state.min(self.nodes.len() - 1);
        while s != 0 && !self.nodes[s].transitions.contains_key(&ch) {
            s = self.nodes[s].failure;
        }
        let next = self.nodes[s].transitions.get(&ch).copied().unwrap_or(0);
        (next, self.nodes[next].reward)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ac(entries: &[(&str, f64)]) -> SupplementAutomaton {
        let owned: Vec<(String, f64)> = entries
            .iter()
            .map(|(w, wt)| (w.to_string(), *wt))
            .collect();
        SupplementAutomaton::build(&owned, 9.0, 2.0, 16.0)
    }

    #[test]
    fn 词中任意位置命中() {
        let a = ac(&[("赢麻了", 1000.0)]);
        // 句子「他们赢麻了吗」——「赢麻了」出现在中段
        let mut s = 0;
        let mut total = 0.0;
        for ch in "他们赢麻了吗".chars() {
            let (ns, r) = a.advance(s, ch);
            s = ns;
            total += r;
        }
        assert!((total - 9.0).abs() < 1e-9, "应命中一次 reward=9，得 {total}");
    }

    #[test]
    fn 两次出现两次计分() {
        let a = ac(&[("真香", 1000.0)]);
        let mut s = 0;
        let mut total = 0.0;
        for ch in "真香真香".chars() {
            let (ns, r) = a.advance(s, ch);
            s = ns;
            total += r;
        }
        assert!((total - 18.0).abs() < 1e-9, "两次命中应 18，得 {total}");
    }

    #[test]
    fn 嵌套词不重复计分() {
        // 「麻了」是「赢麻了」的后缀：完成「赢麻了」的字不重复加「麻了」的份
        let a = ac(&[("赢麻了", 1000.0), ("麻了", 1000.0)]);
        let mut s = 0;
        let mut total = 0.0;
        for ch in "赢麻了".chars() {
            let (ns, r) = a.advance(s, ch);
            s = ns;
            total += r;
        }
        assert!((total - 9.0).abs() < 1e-9, "嵌套后缀只计一次 max，得 {total}");
    }

    #[test]
    fn 权重对数与上限() {
        // w=1000 → 9；w=8000 → 9+2ln8≈13.16；w 超大 clamp 16
        let a = ac(&[("甲", 1000.0), ("乙", 8000.0), ("丙", 1e12)]);
        let (_, r1) = a.advance(0, '甲');
        let (_, r2) = a.advance(0, '乙');
        let (_, r3) = a.advance(0, '丙');
        assert!((r1 - 9.0).abs() < 1e-9);
        assert!((r2 - (9.0 + 2.0 * 8f64.ln())).abs() < 1e-9);
        assert!((r3 - 16.0).abs() < 1e-9);
    }
}
