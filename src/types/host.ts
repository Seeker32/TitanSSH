export interface HostConfig {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authType: AuthType;
  /** 密码在安全存储中的引用键，不含明文 */
  passwordRef?: string;
  privateKeyPath?: string;
  /** 私钥口令在安全存储中的引用键，不含明文 */
  passphraseRef?: string;
  remark?: string;
  /** 分组名，空串表示"未分组" */
  group: string;
}

/** 保存主机请求，含明文凭据，仅用于提交时传递，不得持久化 */
export interface SaveHostRequest {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authType: AuthType;
  password?: string;
  privateKeyPath?: string;
  passphrase?: string;
  remark?: string;
  /** 分组名，空串表示"未分组" */
  group: string;
}

export enum AuthType {
  Password = "Password",
  PrivateKey = "PrivateKey",
}
