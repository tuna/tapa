// Copyright (c) 2024 RapidStream Design Automation, Inc. and contributors.
// All rights reserved. The contributor(s) of this file has/have agreed to the
// RapidStream Contributor License Agreement.

#include <cassert>
#include <cstdio>
#include <cstring>

#include <tapa.h>

#include "sextans-no-const-para.h"
// #include "modules.h"

constexpr int FIFO_DEPTH = 2;
constexpr int PEG_PER_A = 512 / 256;
#define NUM_ITE 2
#define NUM_A_LEN kSextansSparseBeatsPerChannel
#define M kSextansNumRows
#define K kSextansNumRows
#define P_N ((kSextansRepeatCount << 16) | kSextansNumColumns)
#define ALPHA_U 1062836634
#define BETA_U -1073490166

struct MultBVec {
  tapa::u<18> row;
  float_v8 abvec;
};

template <typename T, typename R>
inline void async_read(tapa::async_mmap<T>& A, tapa::ostream<T>& fifo_A,
                       const R A_len, R& i_req, R& i_resp) {
  if ((i_req < A_len) & !A.read_addr.full()) {
    A.read_addr.try_write(i_req);
    ++i_req;
  }
  if (!fifo_A.full() & !A.read_data.empty()) {
    T tmp;
    A.read_data.try_read(tmp);
    fifo_A.try_write(tmp);
    ++i_resp;
  }
}

template <typename T, typename R>
inline void async_read_in_read_edge_list_ptr(tapa::async_mmap<T>& A,
                                             tapa::ostream<T>& fifo_A, R& i_req,
                                             R& i_resp) {
  if ((i_req < NUM_ITE + 1) & !A.read_addr.full()) {
    A.read_addr.try_write(i_req);
    ++i_req;
  }
  if (!fifo_A.full() & !A.read_data.empty()) {
    T tmp;
    A.read_data.try_read(tmp);
    fifo_A.try_write(tmp);
    ++i_resp;
  }
}

template <typename T, typename R>
inline void async_read_in_read_A(tapa::async_mmap<T>& A,
                                 tapa::ostream<T>& fifo_A, R& i_req,
                                 R& i_resp) {
  if ((i_req < NUM_A_LEN) & !A.read_addr.full()) {
    A.read_addr.try_write(i_req);
    ++i_req;
  }
  if (!fifo_A.full() & !A.read_data.empty()) {
    T tmp;
    A.read_data.try_read(tmp);
    fifo_A.try_write(tmp);
    ++i_resp;
  }
}

void read_edge_list_ptr(tapa::async_mmap<int>& edge_list_ptr,
                        tapa::ostream<int>& fifo_edge_list_ptr,
                        tapa::ostream<int>& PE_inst) {
  PE_inst.write(NUM_ITE);
  PE_inst.write(M);
  PE_inst.write(P_N);
  PE_inst.write(K);

  const int N = P_N & 0xFFFF;
  const int N16 = P_N >> 16;
  const int rp_time = (N16 == 0) ? 1 : N16;

  const int num_ite_plus1 = NUM_ITE + 1;
  const int rp_time_N = rp_time * ((N + 7) >> 3);

l_rp:
  [[tapa::tripcount(1, 16)]] [[tapa::flatten(false)]] for (int rp = 0;
                                                           rp < rp_time_N;
                                                           rp++) {
  rd_ptr:
    [[tapa::pipeline(1, "stp")]] [[tapa::tripcount(
        1, 800)]] for (int i_req = 0, i_resp = 0; i_resp < num_ite_plus1;) {
      async_read_in_read_edge_list_ptr(edge_list_ptr, fifo_edge_list_ptr, i_req,
                                       i_resp);
    }
  }
}

void read_A(tapa::async_mmap<tapa::u<512>>& A,
            tapa::ostream<tapa::u<512>>& fifo_A) {
  const int N16 = P_N >> 16;
  const int rp_time = (N16 == 0) ? 1 : N16;
  const int N = P_N & 0xFFFF;

  const int rp_time_N = rp_time * ((N + 7) >> 3);

l_rp:
  [[tapa::tripcount(1, 16)]] [[tapa::flatten(false)]] for (int rp = 0;
                                                           rp < rp_time_N;
                                                           rp++) {
  rd_A:
    [[tapa::pipeline(1, "stp")]] [[tapa::tripcount(
        1, 10000)]] for (int i_req = 0, i_resp = 0; i_resp < NUM_A_LEN;) {
      async_read_in_read_A(A, fifo_A, i_req, i_resp);
    }
  }
}

void read_B(tapa::async_mmap<float_v16>& B, tapa::ostream<float_v16>& fifo_B) {
  const int N16 = P_N >> 16;
  const int rp_time = (N16 == 0) ? 1 : N16;
  const int N = P_N & 0xFFFF;
  const int num_ite_B = ((K + 7) >> 3) * ((N + 7) >> 3);

l_rp:
  [[tapa::tripcount(1, 16)]] [[tapa::flatten(false)]] for (int rp = 0;
                                                           rp < rp_time; rp++) {
  rd_B:
    [[tapa::pipeline(1, "stp")]] [[tapa::tripcount(
        1, 500000)]] for (int i_req = 0, i_resp = 0; i_resp < num_ite_B;) {
      async_read(B, fifo_B, num_ite_B, i_req, i_resp);
    }
  }
}

void read_C(tapa::async_mmap<float_v16>& C, tapa::ostream<float_v16>& fifo_C,
            tapa::ostream<int>& wrC_inst) {
  wrC_inst.write(M);
  wrC_inst.write(P_N);

  const int N16 = P_N >> 16;
  const int rp_time = (N16 == 0) ? 1 : N16;
  const int N = P_N & 0xFFFF;
  const int num_ite_C = ((M + 15) >> 4) * ((N + 7) >> 3);

l_rp:
  [[tapa::tripcount(1, 16)]] [[tapa::flatten(false)]] for (int rp = 0;
                                                           rp < rp_time; rp++) {
  rd_C:
    [[tapa::pipeline(1, "stp")]] [[tapa::tripcount(
        1, 500000)]] for (int i_req = 0, i_resp = 0; i_resp < num_ite_C;) {
      async_read(C, fifo_C, num_ite_C, i_req, i_resp);
    }
  }
}

void write_C(tapa::istream<int>& wrC_inst, tapa::istream<float_v16>& fifo_C,
             tapa::async_mmap<float_v16>& C_out) {
  int local_M = wrC_inst.read();
  int local_P_N = wrC_inst.read();

  const int N16 = local_P_N >> 16;
  const int rp_time = (N16 == 0) ? 1 : N16;
  const int N = local_P_N & 0xFFFF;
  const int num_ite_C = ((local_M + 15) >> 4) * ((N + 7) >> 3);

l_rp:
  [[tapa::tripcount(1, 16)]] [[tapa::flatten(false)]] for (int rp = 0;
                                                           rp < rp_time; rp++) {
  wr_C:
    [[tapa::pipeline(1, "stp")]] [[tapa::tripcount(
        1, 500000)]] for (int i_req = 0, i_resp = 0; i_resp < num_ite_C;) {
      if ((i_req < num_ite_C) & !fifo_C.empty() & !C_out.write_addr.full() &
          !C_out.write_data.full()) {
        C_out.write_addr.try_write(i_req);
        float_v16 tmpv;
        fifo_C.try_read(tmpv);
        C_out.write_data.try_write(tmpv);
        ++i_req;
      }
      uint8_t n_resp;
      if (C_out.write_resp.try_read(n_resp)) {
        i_resp += int(n_resp) + 1;
      }
    }
  }
}

void FloatvMultConst_alpha(tapa::istream<float_v16>& fifo_in,
                           tapa::ostream<float_v16>& fifo_out) {
  const float alpha_f = tapa::bit_cast<float>(ALPHA_U);
  const int N16 = P_N >> 16;
  const int rp_time = (N16 == 0) ? 1 : N16;
  const int N = P_N & 0xFFFF;
  const int num_ite = ((M + 15) >> 4) * ((N + 7) >> 3) * rp_time;
cc:
  [[tapa::pipeline(1, "stp")]] for (int i = 0; i < num_ite;) {
    float_v16 tmp;
    bool read_ready = fifo_in.try_read(tmp);
    if (read_ready) {
      float_v16 c_out = tmp * alpha_f;
      fifo_out.write(c_out);
      ++i;
    }
  }
}

void FloatvMultConst_beta(tapa::istream<float_v16>& fifo_in,
                          tapa::ostream<float_v16>& fifo_out) {
  const float alpha_f = tapa::bit_cast<float>(BETA_U);
  const int N16 = P_N >> 16;
  const int rp_time = (N16 == 0) ? 1 : N16;
  const int N = P_N & 0xFFFF;
  const int num_ite = ((M + 15) >> 4) * ((N + 7) >> 3) * rp_time;
cc:
  [[tapa::pipeline(1, "stp")]] for (int i = 0; i < num_ite;) {
    float_v16 tmp;
    bool read_ready = fifo_in.try_read(tmp);
    if (read_ready) {
      float_v16 c_out = tmp * alpha_f;
      fifo_out.write(c_out);
      ++i;
    }
  }
}

void FloatvAddFloatv(tapa::istream<float_v16>& fifo_in0,
                     tapa::istream<float_v16>& fifo_in1,
                     tapa::ostream<float_v16>& fifo_out) {
cc:
  [[tapa::pipeline(1, "stp")]] for (;;) {
    bool flag_nop = fifo_in0.empty() | fifo_in1.empty();
    if (!flag_nop) {
      float_v16 tmp0;
      fifo_in0.try_read(tmp0);
      float_v16 tmp1;
      fifo_in1.try_read(tmp1);
      float_v16 c_out = tmp0 + tmp1;
      fifo_out.write(c_out);
    }
  }
}

void PEcore_Bmtx(tapa::u<14> addr_b, tapa::u<32> a_val_u,
                 float local_B[8][WINDOW_SIZE], float_v8& abv) {
  float a_val_f = tapa::bit_cast<float>(a_val_u);
  for (int i = 0; i < 8; ++i) {
    abv[i] = a_val_f * local_B[i][addr_b];
  }
}

void PEG_Bmtx(
    tapa::istream<int>& PE_inst_in, tapa::istream<int>& fifo_inst_in,
    // tapa::istream<tapa::u<128>> & fifo_A,
    tapa::istream<tapa::u<256>>& fifo_A,
    tapa::istreams<float_v16, NUM_CH_B>& fifo_B_in,  // [256(16)] * 2, 2: dim d
    // [64(32bits * 2.0)] * 8 dim
    tapa::ostream<int>& PE_inst_out, tapa::ostream<int>& fifo_inst_out,
    tapa::ostreams<float_v16, NUM_CH_B>& fifo_B_out,
    // to PEG_Cmtx
    tapa::ostream<int>& PE_inst_to_Cmtx,
    tapa::ostream<int>& fifo_inst_out_to_Cmtx,
    tapa::ostreams<MultBVec, 4>& fifo_aBvec) {
  const int local_NUM_ITE = PE_inst_in.read();
  const int local_M = PE_inst_in.read();
  const int local_P_N = PE_inst_in.read();
  const int local_K = PE_inst_in.read();

  PE_inst_out.write(local_NUM_ITE);
  PE_inst_out.write(local_M);
  PE_inst_out.write(local_P_N);
  PE_inst_out.write(local_K);

  PE_inst_to_Cmtx.write(local_NUM_ITE);
  PE_inst_to_Cmtx.write(local_M);
  PE_inst_to_Cmtx.write(local_P_N);

  const int N16 = local_P_N >> 16;
  const int rp_time = (N16 == 0) ? 1 : N16;
  const int N = local_P_N & 0xFFFF;
  const int rp_time_N = rp_time * ((N + 7) >> 3);

l_rp:
  [[tapa::tripcount(1, 16)]] [[tapa::flatten(false)]] for (int rp = 0;
                                                           rp < rp_time_N;
                                                           rp++) {
    // float local_B[8/2][8][WINDOW_SIZE];
    // float local_B[8][WINDOW_SIZE];
    [[tapa::storage("RAM_2P", "BRAM", 2)]] [[tapa::partition(
        "complete", -1,
        1)]] [[tapa::partition("complete", -1,
                               2)]] [[tapa::partition("cyclic",
                                                      B_PARTITION_FACTOR,
                                                      3)]] float
        local_B[4 / 2][8][WINDOW_SIZE];
    // To avoid auto memory type inference, previously
    // PEG_Bmtx_local_B_RAM_AUTO_1R1W
    // #pragma HLS array_partition variable=local_B cyclic
    // factor=B_PARTITION_FACTOR dim=2

    auto start_32 = fifo_inst_in.read();
    fifo_inst_out.write(start_32);
    fifo_inst_out_to_Cmtx.write(start_32);

  main:
    [[tapa::tripcount(1, 49)]] for (int i = 0; i < local_NUM_ITE; ++i) {
      // fill onchip B
    read_B:
      [[tapa::pipeline(1, "stp")]] [[tapa::tripcount(
          1, 512)]] for (int j = 0;
                         (j < (WINDOW_SIZE >> 3)) &&
                         (j < ((local_K + 7) >> 3) - i * (WINDOW_SIZE >> 3));) {
        bool b_2048_ready = true;
        bool b_2048_out_not_full = true;
        for (int k = 0; k < NUM_CH_B; ++k) {
          b_2048_ready &= !fifo_B_in[k].empty();
          b_2048_out_not_full &= !fifo_B_out[k].full();
        }

        if (b_2048_ready & b_2048_out_not_full) {
          float_v16 b_512_x[NUM_CH_B];
          for (int k = 0; k < NUM_CH_B; ++k) {
            fifo_B_in[k].try_read(b_512_x[k]);
            fifo_B_out[k].try_write(b_512_x[k]);
          }

          for (int k = 0; k < 8; ++k) {
            for (int m = 0; m < 8; ++m) {
              for (int l = 0; l < 2; ++l) {
                local_B[l][m][j * 8 + k] = b_512_x[m / 2][k + m % 2 * 8];
              }
            }
          }
          ++j;
        }
      }

      // computation
      const auto end_32 = fifo_inst_in.read();
      fifo_inst_out.write(end_32);
      fifo_inst_out_to_Cmtx.write(end_32);

    computation:
      [[tapa::pipeline(1, "stp")]] [[tapa::tripcount(
          1, 200)]] for (int j = start_32; j < end_32;) {
        // tapa::u<128> a_pes;
        tapa::u<256> a_pes;
        bool a_pes_ready = fifo_A.try_read(a_pes);

        if (a_pes_ready) {
          for (int p = 0; p < 4; ++p) {
            tapa::u<64> a = a_pes(63 + p * 64, p * 64);

            tapa::u<14> a_col = a(63, 50);
            tapa::u<18> a_row = a(49, 32);
            tapa::u<32> a_val = a(31, 0);

            MultBVec rabv;
            rabv.row = a_row;

            if (a_row[17] == 0) {
              // PE process
              PEcore_Bmtx(a_col, a_val, local_B[p / 2], rabv.abvec);
            }
            fifo_aBvec[p].write(rabv);
          }
          ++j;
        }
      }
      start_32 = end_32;
    }
  }
}

void PU2core_Cmtx(tapa::u<18> addr_c, float val_d0_f, float val_d1_f,
                  tapa::u<64> local_C_pe0_d0_d1[URAM_DEPTH]) {
  tapa::u<64> c_val_d0_d1_u64 = local_C_pe0_d0_d1[addr_c];

  tapa::u<32> c_val_d0_u = c_val_d0_d1_u64(31, 0);
  tapa::u<32> c_val_d1_u = c_val_d0_d1_u64(63, 32);

  float c_val_d0_f = tapa::bit_cast<float>(c_val_d0_u) + val_d0_f;
  float c_val_d1_f = tapa::bit_cast<float>(c_val_d1_u) + val_d1_f;

  c_val_d0_u = tapa::bit_cast<tapa::u<32>>(c_val_d0_f);
  c_val_d1_u = tapa::bit_cast<tapa::u<32>>(c_val_d1_f);

  c_val_d0_d1_u64(31, 0) = c_val_d0_u;
  c_val_d0_d1_u64(63, 32) = c_val_d1_u;

  local_C_pe0_d0_d1[addr_c] = c_val_d0_d1_u64;
}

void PEcore_Cmtx(tapa::u<18> addr_c, float_v8& abvec,
                 tapa::u<64> local_C[4][URAM_DEPTH]) {
  for (int i = 0; i < 4; ++i) {
    PU2core_Cmtx(addr_c, abvec[i * 2 + 0], abvec[i * 2 + 1], local_C[i]);
  }
}

void PEG_Cmtx(tapa::istream<int>& PE_inst_in, tapa::istream<int>& fifo_inst_in,
              tapa::istreams<MultBVec, 4>& fifo_aBvec,
              tapa::ostream<float_v8>& fifo_C_out) {
  const int local_NUM_ITE = PE_inst_in.read();
  const int local_M = PE_inst_in.read();
  const int local_P_N = PE_inst_in.read();

  const int N16 = local_P_N >> 16;
  const int rp_time = (N16 == 0) ? 1 : N16;
  const int N = local_P_N & 0xFFFF;
  const int rp_time_N = rp_time * ((N + 7) >> 3);

  const int num_v_init = (local_M + 63) >> 6;
  // const int num_v_out = (local_M + 31) >> 5;
  const int num_v_out = (local_M + 15) >> 4;

  // define local C buffer and pragma to URAM
  // tapa::u<64> local_C[2][8 / 2][URAM_DEPTH];
  [[tapa::storage("RAM_2P", "URAM", 1)]] [[tapa::partition(
      "complete", -1,
      1)]] [[tapa::partition("complete", -1,
                             2)]] tapa::u<64> local_C[4][8 / 2][URAM_DEPTH];

l_rp:
  [[tapa::tripcount(1, 16)]] [[tapa::flatten(false)]] for (int rp = 0;
                                                           rp < rp_time_N;
                                                           rp++) {
    // init local C
  init_C:
    [[tapa::pipeline(1, "stp")]] [[tapa::tripcount(
        1, 800)]] for (int i = 0; i < num_v_init; ++i) {
      // for (int j = 0; j < 2; ++j) {
      for (int j = 0; j < 4; ++j) {
        for (int k = 0; k < 8 / 2; ++k) {
          local_C[j][k][i] = 0;
        }
      }
    }

    auto start_32 = fifo_inst_in.read();

  main:
    [[tapa::tripcount(1, 49)]] for (int i = 0; i < local_NUM_ITE; ++i) {
      // computation
      const auto end_32 = fifo_inst_in.read();

    computation:
      [[tapa::dependence(
          "local_C", "", "", "", 1,
          "DEP_DIST_LOAD_STORE")]] [[tapa::
                                         pipeline(1, "stp")]] [[tapa::tripcount(
          1, 200)]] for (int j = start_32; j < end_32;) {
        bool nop_flag = false;
        for (int p = 0; p < 4; ++p) {
          nop_flag |= fifo_aBvec[p].empty();
        }

        if (!nop_flag) {
          for (int p = 0; p < 4; ++p) {
            MultBVec rabv;
            fifo_aBvec[p].try_read(rabv);
            tapa::u<18> a_row = rabv.row;

            if (a_row[17] == 0) {
              PEcore_Cmtx(a_row, rabv.abvec, local_C[p]);
            }
          }
          ++j;
        }
      }
      start_32 = end_32;
    }

    // cout << "PE = " << pe_idx << endl;
  write_C_outer:
    [[tapa::pipeline(1, "stp")]] [[tapa::tripcount(
        1, 1800)]] for (int i = 0, c_idx = 0; i < num_v_out; ++i) {
      tapa::u<32> u_32_d[8];

      for (int d = 0; d < 4; ++d) {
        tapa::u<64> u_64 = local_C[c_idx][d][i >> 2];
        u_32_d[2 * d] = u_64(31, 0);
        u_32_d[2 * d + 1] = u_64(63, 32);
      }

      switch (c_idx) {  // 0,2,1,3
        case 0:
          c_idx = 2;
          break;
        case 1:
          c_idx = 3;
          break;
        case 2:
          c_idx = 1;
          break;
        case 3:
          c_idx = 0;
          break;
      }

      float_v8 out_v;
      for (int d = 0; d < 8; ++d) {
        out_v[d] = tapa::bit_cast<float>(u_32_d[d]);
      }
      fifo_C_out.write(out_v);
      // for (int ii = 0; ii < 8; ++ii) {cout << out_v[ii] << " ";} cout <<
      // endl;
    }
  }
}

void Scatter_1_2(tapa::istream<tapa::u<512>>& fifo_in,
                 tapa::ostreams<tapa::u<256>, 2>& fifo_out) {
  [[tapa::pipeline(1, "stp")]] for (;;) {
    bool flag_nop = fifo_in.empty();
    for (int i = 0; i < 2; ++i) {
      flag_nop |= fifo_out[i].full();
    }
    if (!flag_nop) {
      tapa::u<512> tmp;
      fifo_in.try_read(tmp);
      for (int i = 0; i < 2; ++i) {
        fifo_out[i].try_write(tmp(255 + i * 256, i * 256));
      }
    }
  }
}

void Merger(tapa::istreams<float_v8, 2>& fifo_in,
            tapa::ostream<float_v16>& fifo_out) {
  [[tapa::pipeline(1, "stp")]] for (;;) {
    bool flag_nop = fifo_out.full() | fifo_in[0].empty() | fifo_in[1].empty();
    if (!flag_nop) {
      [[tapa::aggregate]] float_v16 tmpv16;
      float_v8 tmpv8[2];
      fifo_in[0].try_read(tmpv8[0]);
      fifo_in[1].try_read(tmpv8[1]);
      for (int i = 0; i < 8; ++i) {
        tmpv16[i] = tmpv8[0][i];
        tmpv16[i + 8] = tmpv8[1][i];
      }
      fifo_out.try_write(tmpv16);
    }
  }
}

void black_hole_int(tapa::istream<int>& fifo_in) {
  [[tapa::pipeline(1, "stp")]] for (;;) { fifo_in.read(nullptr); }
}

void black_hole_float_v16(tapa::istream<float_v16>& fifo_in) {
  [[tapa::pipeline(1, "stp")]] for (;;) { fifo_in.read(nullptr); }
}

void Sextans(tapa::mmap<int> edge_list_ptr,

             tapa::mmaps<tapa::u<512>, NUM_CH_SPARSE> edge_list_ch,

             tapa::mmaps<float_v16, NUM_CH_B> mat_B_ch,

             tapa::mmaps<float_v16, NUM_CH_C> mat_C_ch_in,

             tapa::mmaps<float_v16, NUM_CH_C> mat_C_ch) {
  tapa::streams<int, NUM_CH_SPARSE * PEG_PER_A + 1, FIFO_DEPTH> PE_inst(
      "PE_inst");

  tapa::streams<int, NUM_CH_C, FIFO_DEPTH> wrC_inst("wrC_inst");

  tapa::streams<int, NUM_CH_SPARSE * PEG_PER_A + 1, FIFO_DEPTH>
      fifo_edge_list_ptr("fifo_edge_list_ptr");

  tapa::streams<int, NUM_CH_SPARSE * PEG_PER_A, FIFO_DEPTH> PE_inst_to_Cmtx(
      "PE_inst_to_Cmtx");

  tapa::streams<int, NUM_CH_SPARSE * PEG_PER_A, FIFO_DEPTH>
      fifo_edge_list_ptr_to_Cmtx("fifo_edge_list_ptr_to_Cmtx");

  /* ============================== */

  tapa::streams<tapa::u<512>, NUM_CH_SPARSE, FIFO_DEPTH> fifo_A("fifo_A");

  tapa::streams<tapa::u<256>, NUM_CH_SPARSE * PEG_PER_A, FIFO_DEPTH> fifo_A_pe(
      "fifo_A_pe");

  tapa::streams<float_v16, (NUM_CH_SPARSE * PEG_PER_A + 1) * NUM_CH_B,
                FIFO_DEPTH>
      fifo_B_pe("fifo_B_pe");

  tapa::streams<float_v8, NUM_CH_SPARSE * PEG_PER_A, FIFO_DEPTH> fifo_C_pe(
      "fifo_C_pe");

  tapa::streams<MultBVec, NUM_CH_SPARSE * PEG_PER_A * 4, FIFO_DEPTH> fifo_aBvec(
      "fifo_aBvec");

  tapa::streams<float_v16, NUM_CH_C, FIFO_DEPTH> fifo_C_read_in(
      "fifo_C_read_in");

  tapa::streams<float_v16, NUM_CH_C, FIFO_DEPTH> fifo_C_read_in_beta(
      "fifo_C_read_in_beta");

  tapa::streams<float_v16, NUM_CH_C, FIFO_DEPTH> fifo_C_ch_result(
      "fifo_C_ch_result");

  tapa::streams<float_v16, NUM_CH_C, FIFO_DEPTH> fifo_C_ch_result_alpha(
      "fifo_C_ch_result_alpha");

  tapa::streams<float_v16, (uint64_t)NUM_CH_C, (uint64_t)FIFO_DEPTH> fifo_C_ch(
      "fifo_C_ch");

  tapa::task()
      .invoke(read_edge_list_ptr, edge_list_ptr, fifo_edge_list_ptr, PE_inst)

      .invoke<tapa::join, NUM_CH_SPARSE>(read_A, edge_list_ch, fifo_A)

      .invoke<tapa::detach, NUM_CH_SPARSE>(Scatter_1_2, fifo_A, fifo_A_pe)

      .invoke<tapa::join, NUM_CH_B>(read_B, mat_B_ch, fifo_B_pe)

      .invoke<tapa::join, NUM_CH_SPARSE * PEG_PER_A>(
          PEG_Bmtx, PE_inst, fifo_edge_list_ptr, fifo_A_pe, fifo_B_pe, PE_inst,
          fifo_edge_list_ptr, fifo_B_pe, PE_inst_to_Cmtx,
          fifo_edge_list_ptr_to_Cmtx, fifo_aBvec)

      .invoke<tapa::join, NUM_CH_SPARSE * PEG_PER_A>(PEG_Cmtx, PE_inst_to_Cmtx,
                                                     fifo_edge_list_ptr_to_Cmtx,
                                                     fifo_aBvec, fifo_C_pe)

      .invoke<tapa::detach>(black_hole_int, PE_inst)
      .invoke<tapa::detach>(black_hole_int, fifo_edge_list_ptr)
      .invoke<tapa::detach, NUM_CH_B>(black_hole_float_v16, fifo_B_pe)

      .invoke<tapa::detach, NUM_CH_SPARSE>(Merger, fifo_C_pe, fifo_C_ch_result)

      .invoke<tapa::join, NUM_CH_C>(read_C, mat_C_ch_in, fifo_C_read_in,
                                    wrC_inst)

      .invoke<tapa::join, NUM_CH_C>(FloatvMultConst_beta, fifo_C_read_in,
                                    fifo_C_read_in_beta)

      .invoke<tapa::join, NUM_CH_C>(FloatvMultConst_alpha, fifo_C_ch_result,
                                    fifo_C_ch_result_alpha)

      .invoke<tapa::detach, NUM_CH_C>(FloatvAddFloatv, fifo_C_ch_result_alpha,
                                      fifo_C_read_in_beta, fifo_C_ch)

      .invoke<tapa::join, NUM_CH_C>(write_C, wrC_inst, fifo_C_ch, mat_C_ch);
}
