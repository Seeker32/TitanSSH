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

/** 三态凭据输入:字符串=设置新值(空串=保持旧值),{clear:true}=显式清除,缺失/null=保持旧值 */
export type CredentialInput = string | { clear: true };

/** 保存主机请求，含明文凭据，仅用于提交时传递，不得持久化 */
export interface SaveHostRequest {
  id: string;
  name: string;
  host: string;
  port: number;
  username: string;
  authType: AuthType;
  password?: CredentialInput | null;
  privateKeyPath?: string;
  passphrase?: CredentialInput | null;
  remark?: string;
  /** 分组名，空串表示"未分组" */
  group: string;
}

export enum AuthType {
  Password = "Password",
  PrivateKey = "PrivateKey",
}
