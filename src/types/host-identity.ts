/** Settings“可信主机”只读清单条目（后端 TrustedHostInfo 的 typed JSON 投影）。
 *  原始公钥 material 只存在后端；前端不解析 known_hosts 文本。 */
export interface TrustedHostInfo {
  host: string;
  port: number;
  algorithm: string;
  fingerprint: string;
}
