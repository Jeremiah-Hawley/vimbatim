#include <fstream>
using std::ifstream;

#include <deque>
using std::deque;

#include "run.hpp"


class paragraph{
  public:
    paragraph(){ //defult constructor
      type=3;
      heading=0;
    }

    paragraph(ifstream document, int index, char tp){ //from document, index is which paragraph to pull from
      type=tp;
      
    }

    paragraph(string xml){ //from paragraph xml string

    }

    void set_type(char t){ type=t; }
    void set_heading(char lv){ heading=lv; }
    void add_run(run line){ runs.push_back(line); }

    char get_type(){ return type; }
    char get_heading(){ return heading; }
    run get_run(int index){ return runs[index]; }

    void insert_run(run line, int location){
      auto it = runs.begin() + location;
      runs.insert(it, line);
    }

    string to_xml(){
      string xml = "<w:p><w:pPr>";
      if(heading!=0){
        xml += "<pStyle w=val=\"Heading" + (int)heading + "\"/>";
      }
      xml += "</w:pPr>";
      for(run line : runs){
        xml += line.to_xml();
      }
      xml += "</w:p>";
      return xml;
    }

    


  private:
    char type; //tag = 0, cite = 1, metadata = 2, text = 3, card = 4
    deque<run> runs; //array of runs
    char heading; // this is the only format that transends runs
}
